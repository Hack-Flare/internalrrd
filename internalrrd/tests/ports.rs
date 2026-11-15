use std::error::Error;

use internalrrd::config::PortRangeConfig;
use internalrrd::ports::PortAllocator;

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
async fn allocates_multiple_distinct_ports() -> Result<(), Box<dyn Error>> {
    let range = PortRangeConfig {
        start: 40010,
        end: 40012,
    };
    let mut allocator = PortAllocator::new(Some(&range), []);

    let ports = allocator.allocate_many(3).await?;

    assert_eq!(ports, vec![40010, 40011, 40012]);
    assert!(allocator.allocate().await.is_err());
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
    let range = PortRangeConfig {
        start: 40020,
        end: 40020,
    };
    let mut allocator = PortAllocator::new(Some(&range), []);

    let port = allocator.allocate().await?;
    assert_eq!(port, 40020);
    assert!(allocator.allocate().await.is_err());

    allocator.release(port);
    assert_eq!(allocator.allocate().await?, port);
    Ok(())
}
