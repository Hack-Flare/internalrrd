use std::collections::BTreeSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use tokio::net::{TcpListener, UdpSocket};

use crate::config::PortRangeConfig;

const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Allocates private ports for application instances.
///
/// Ports are checked for availability on TCP/UDP so an instance can use either
#[derive(Debug)]
pub struct PortAllocator {
    range: Option<(u16, u16)>,
    reserved: BTreeSet<u16>,
}

impl PortAllocator {
    pub fn new(range: Option<&PortRangeConfig>, reserved: impl IntoIterator<Item = u16>) -> Self {
        let mut reserved: BTreeSet<u16> = reserved.into_iter().collect();
        reserved.remove(&0);

        Self {
            range: range.map(|range| (range.start, range.end)),
            reserved,
        }
    }

    /// Allocate a free port and reserve it until it is released.
    pub async fn allocate(&mut self) -> Result<u16, PortError> {
        match self.range {
            Some((start, end)) => {
                for port in start..=end {
                    if self.reserved.contains(&port) {
                        continue;
                    }
                    if can_bind(port).await {
                        self.reserved.insert(port);
                        return Ok(port);
                    }
                }
                Err(PortError::Exhausted)
            }
            None => {
                for _ in 0..64 {
                    let port = ephemeral().await?;
                    if !self.reserved.contains(&port) && can_bind(port).await {
                        self.reserved.insert(port);
                        return Ok(port);
                    }
                }
                Err(PortError::Exhausted)
            }
        }
    }

    /// Release a port so that it can be allocated again.
    pub fn release(&mut self, port: u16) {
        self.reserved.remove(&port);
    }
}

async fn can_bind(port: u16) -> bool {
    let address = SocketAddr::new(LOCAL, port);
    let tcp = TcpListener::bind(address).await;
    let udp = UdpSocket::bind(address).await;

    matches!((tcp, udp), (Ok(_), Ok(_)))
}

async fn ephemeral() -> Result<u16, PortError> {
    let listener = TcpListener::bind(SocketAddr::new(LOCAL, 0))
        .await
        .map_err(PortError::Bind)?;
    let port = listener.local_addr().map_err(PortError::Bind)?.port();
    Ok(port)
}

#[derive(Debug, thiserror::Error)]
pub enum PortError {
    #[error("failed to bind a temporary socket while allocating a port: {0}")]
    Bind(#[source] io::Error),
    #[error("no free application ports remain")]
    Exhausted,
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[tokio::test]
    async fn allocates_distinct_ephemeral_ports() -> Result<(), Box<dyn Error>> {
        let mut allocator = PortAllocator::new(None, []);

        let first = allocator.allocate().await?;
        let second = allocator.allocate().await?;

        assert_ne!(first, second);
        assert!(first > 0);
        assert!(second > 0);
        Ok(())
    }

    #[tokio::test]
    async fn skips_reserved_ports() -> Result<(), Box<dyn Error>> {
        let range = PortRangeConfig {
            start: 40000,
            end: 40002,
        };
        let mut allocator = PortAllocator::new(Some(&range), [40000, 40001]);

        let port = allocator.allocate().await?;

        assert_eq!(port, 40002);
        Ok(())
    }

    #[tokio::test]
    async fn releases_ports() -> Result<(), Box<dyn Error>> {
        let mut allocator = PortAllocator::new(None, []);

        let port = allocator.allocate().await?;
        assert!(allocator.reserved.contains(&port));

        allocator.release(port);
        assert!(!allocator.reserved.contains(&port));
        assert!(allocator.allocate().await? > 0);
        Ok(())
    }
}
