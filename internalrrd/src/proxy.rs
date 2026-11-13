use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::copy_bidirectional;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const UDP_BUFFER_BYTES: usize = 65_535;
const UDP_SESSION_QUEUE: usize = 16;

/// The transport a public listener accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Tcp,
    Udp,
}

/// One public listener that forwards to the active application instances.
#[derive(Debug, Clone)]
pub struct ListenerSpec {
    pub transport: Transport,
    pub bind: IpAddr,
    pub port: u16,
}

/// An L4 proxy whose backends can be switched atomically.
///
/// TCP connections and UDP sessions that are already open keep using the backend hey started with.
/// New connections and sessions use the latest backend set.
pub struct Proxy {
    backends: watch::Sender<Arc<Vec<SocketAddr>>>,
    listeners: Vec<BoundListener>,
}

struct BoundListener {
    address: SocketAddr,
    task: JoinHandle<()>,
}

impl Proxy {
    /// Bind every public listener and start serving.
    pub async fn start(
        specs: &[ListenerSpec],
        udp_session_timeout: Duration,
    ) -> Result<Self, ProxyError> {
        let (backends, receiver) = watch::channel(Arc::new(Vec::new()));
        let mut listeners = Vec::with_capacity(specs.len());

        for spec in specs {
            let requested = SocketAddr::new(spec.bind, spec.port);
            let (address, task) = match spec.transport {
                Transport::Tcp => {
                    let listener =
                        TcpListener::bind(requested)
                            .await
                            .map_err(|source| ProxyError::Bind {
                                address: requested,
                                source,
                            })?;
                    let address = listener.local_addr().map_err(|source| ProxyError::Bind {
                        address: requested,
                        source,
                    })?;
                    let task = tokio::spawn(serve_tcp(listener, receiver.clone()));
                    (address, task)
                }
                Transport::Udp => {
                    let socket = Arc::new(UdpSocket::bind(requested).await.map_err(|source| {
                        ProxyError::Bind {
                            address: requested,
                            source,
                        }
                    })?);
                    let address = socket.local_addr().map_err(|source| ProxyError::Bind {
                        address: requested,
                        source,
                    })?;
                    let task =
                        tokio::spawn(serve_udp(socket, receiver.clone(), udp_session_timeout));
                    (address, task)
                }
            };

            listeners.push(BoundListener { address, task });
        }

        Ok(Self {
            backends,
            listeners,
        })
    }

    /// Replace the backend set used for new connections and sessions.
    pub fn switch(&self, backends: Vec<SocketAddr>) {
        if self.backends.send(Arc::new(backends)).is_err() {
            tracing::warn!("proxy has no active listeners to switch");
        }
    }

    /// The addresses that the listeners are bound to.
    pub fn addresses(&self) -> impl Iterator<Item = SocketAddr> + '_ {
        self.listeners.iter().map(|listener| listener.address)
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        for listener in &self.listeners {
            listener.task.abort();
        }
    }
}

async fn serve_tcp(listener: TcpListener, backends: watch::Receiver<Arc<Vec<SocketAddr>>>) {
    let counter = AtomicUsize::new(0);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(error = %error, "proxy TCP accept failed");
                continue;
            }
        };

        let current = backends.borrow().clone();
        let Some(backend) = pick(&current, &counter) else {
            tracing::warn!(%peer, "no backend available for TCP connection");
            continue;
        };

        tokio::spawn(async move {
            let mut upstream = match TcpStream::connect(backend).await {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%peer, %backend, error = %error, "failed to connect to backend");
                    return;
                }
            };
            let mut downstream = stream;

            if let Err(error) = copy_bidirectional(&mut downstream, &mut upstream).await {
                tracing::debug!(
                    %peer,
                    %backend,
                    error = %error,
                    "proxy TCP connection ended with an error"
                );
            }
        });
    }
}

async fn serve_udp(
    socket: Arc<UdpSocket>,
    backends: watch::Receiver<Arc<Vec<SocketAddr>>>,
    idle_timeout: Duration,
) {
    let counter = AtomicUsize::new(0);
    let sessions: Arc<tokio::sync::Mutex<HashMap<SocketAddr, mpsc::Sender<Vec<u8>>>>> =
        Arc::default();
    let mut buffer = vec![0u8; UDP_BUFFER_BYTES];
    let mut prune = tokio::time::interval(idle_timeout.max(Duration::from_secs(1)));

    loop {
        tokio::select! {
            received = socket.recv_from(&mut buffer) => {
                let (length, client) = match received {
                    Ok(datagram) => datagram,
                    Err(error) => {
                        tracing::warn!(error = %error, "proxy UDP receive failed");
                        continue;
                    }
                };
                let payload = buffer[..length].to_vec();

                let sender = {
                    let mut sessions = sessions.lock().await;
                    match sessions.get(&client) {
                        Some(sender) if !sender.is_closed() => sender.clone(),
                        _ => {
                            let current = backends.borrow().clone();
                            let Some(backend) = pick(&current, &counter) else {
                                tracing::warn!(%client, "no backend available for UDP session");
                                continue;
                            };
                            let (sender, receiver) = mpsc::channel(UDP_SESSION_QUEUE);
                            tokio::spawn(serve_udp_session(
                                socket.clone(),
                                client,
                                backend,
                                receiver,
                                idle_timeout,
                            ));
                            sessions.insert(client, sender.clone());
                            sender
                        }
                    }
                };

                if sender.try_send(payload).is_err() {
                    tracing::debug!(%client, "dropping UDP datagram for a busy session");
                }
            }
            _ = prune.tick() => {
                let mut sessions = sessions.lock().await;
                sessions.retain(|_, sender| !sender.is_closed());
            }
        }
    }
}

async fn serve_udp_session(
    front: Arc<UdpSocket>,
    client: SocketAddr,
    backend: SocketAddr,
    mut outgoing: mpsc::Receiver<Vec<u8>>,
    idle_timeout: Duration,
) {
    let bind_address = SocketAddr::new(backend.ip(), 0);
    let backend_socket = match UdpSocket::bind(bind_address).await {
        Ok(socket) => socket,
        Err(error) => {
            tracing::warn!(%client, %backend, error = %error, "failed to bind UDP backend socket");
            return;
        }
    };
    if let Err(error) = backend_socket.connect(backend).await {
        tracing::warn!(%client, %backend, error = %error, "failed to connect UDP backend socket");
        return;
    }

    let mut buffer = vec![0u8; UDP_BUFFER_BYTES];
    let mut deadline = tokio::time::Instant::now() + idle_timeout;

    loop {
        tokio::select! {
            biased;
            Some(payload) = outgoing.recv() => {
                if let Err(error) = backend_socket.send(&payload).await {
                    tracing::debug!(%client, %backend, error = %error, "failed to forward UDP datagram to backend");
                    break;
                }
                deadline = tokio::time::Instant::now() + idle_timeout;
            }
            received = backend_socket.recv(&mut buffer) => {
                match received {
                    Ok(length) => {
                        if let Err(error) = front.send_to(&buffer[..length], client).await {
                            tracing::debug!(%client, error = %error, "failed to forward UDP datagram to client");
                            break;
                        }
                        deadline = tokio::time::Instant::now() + idle_timeout;
                    }
                    Err(error) => {
                        tracing::debug!(%client, %backend, error = %error, "failed to receive UDP datagram from backend");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep_until(deadline) => break,
        }
    }
}

fn pick(backends: &[SocketAddr], counter: &AtomicUsize) -> Option<SocketAddr> {
    if backends.is_empty() {
        return None;
    }

    let index = counter.fetch_add(1, Ordering::Relaxed) % backends.len();
    backends.get(index).copied()
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("failed to bind proxy listener on {address}: {source}")]
    Bind {
        address: SocketAddr,
        source: io::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::net::{Ipv4Addr, SocketAddr};

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream, UdpSocket};

    use super::*;

    fn local(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    #[tokio::test]
    async fn proxies_tcp_connections() -> Result<(), Box<dyn Error>> {
        let backend = TcpListener::bind(local(0)).await?;
        let backend_address = backend.local_addr()?;
        let echo = tokio::spawn(async move {
            if let Ok((mut stream, _)) = backend.accept().await {
                let mut buffer = [0u8; 64];
                if let Ok(length) = stream.read(&mut buffer).await {
                    let _ = stream.write_all(&buffer[..length]).await;
                }
            }
        });

        let proxy = Proxy::start(
            &[ListenerSpec {
                transport: Transport::Tcp,
                bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
            }],
            Duration::from_secs(1),
        )
        .await?;
        let front = proxy
            .addresses()
            .next()
            .ok_or("proxy did not bind a listener")?;
        proxy.switch(vec![backend_address]);

        let mut client = TcpStream::connect(front).await?;
        client.write_all(b"ping").await?;
        let mut response = [0u8; 4];
        client.read_exact(&mut response).await?;

        assert_eq!(&response, b"ping");
        echo.await?;
        Ok(())
    }

    #[tokio::test]
    async fn proxies_udp_datagrams() -> Result<(), Box<dyn Error>> {
        let backend = UdpSocket::bind(local(0)).await?;
        let backend_address = backend.local_addr()?;
        let echo = tokio::spawn(async move {
            let mut buffer = [0u8; 64];
            if let Ok((length, client)) = backend.recv_from(&mut buffer).await {
                let _ = backend.send_to(&buffer[..length], client).await;
            }
        });

        let proxy = Proxy::start(
            &[ListenerSpec {
                transport: Transport::Udp,
                bind: IpAddr::V4(Ipv4Addr::LOCALHOST),
                port: 0,
            }],
            Duration::from_secs(1),
        )
        .await?;
        let front = proxy
            .addresses()
            .next()
            .ok_or("proxy did not bind a listener")?;
        proxy.switch(vec![backend_address]);

        let client = UdpSocket::bind(local(0)).await?;
        client.send_to(b"ping", front).await?;

        let mut response = [0u8; 64];
        let (length, source) =
            tokio::time::timeout(Duration::from_secs(2), client.recv_from(&mut response)).await??;

        assert_eq!(&response[..length], b"ping");
        assert_eq!(source, front);
        echo.await?;
        Ok(())
    }
}
