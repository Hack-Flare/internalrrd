use std::collections::BTreeMap;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use internalrrd::proxy::{ListenerSpec, Proxy, Transport};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};

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
            backend: "default".to_string(),
        }],
        Duration::from_secs(1),
    )
    .await?;
    let front = proxy
        .addresses()
        .next()
        .ok_or("proxy did not bind a listener")?;
    proxy.switch(BTreeMap::from([(
        "default".to_string(),
        vec![backend_address],
    )]));

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
            backend: "default".to_string(),
        }],
        Duration::from_secs(1),
    )
    .await?;
    let front = proxy
        .addresses()
        .next()
        .ok_or("proxy did not bind a listener")?;
    proxy.switch(BTreeMap::from([(
        "default".to_string(),
        vec![backend_address],
    )]));

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
