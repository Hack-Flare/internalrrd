#![cfg(unix)]

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::time::Duration;

use internalrrd::process::ProcessSpec;
use internalrrd::supervisor::Supervisor;

fn sleeping_spec() -> ProcessSpec {
    ProcessSpec {
        command: PathBuf::from("sh"),
        args: vec!["-c".to_string(), "sleep 30".to_string()],
        working_directory: None,
        environment: BTreeMap::new(),
        shutdown_timeout: Duration::from_secs(5),
    }
}

#[tokio::test]
async fn starts_and_stops_instances() -> Result<(), Box<dyn Error>> {
    let mut supervisor = Supervisor::new();

    let first = supervisor.start_with(sleeping_spec())?;
    let second = supervisor.start_with(sleeping_spec())?;

    assert_ne!(first, second);
    assert_eq!(supervisor.running().len(), 2);
    assert!(supervisor.is_running(first));

    supervisor.stop_all().await?;

    assert!(supervisor.running().is_empty());
    assert!(!supervisor.is_running(first));
    Ok(())
}

#[tokio::test]
async fn reports_unknown_instances() -> Result<(), Box<dyn Error>> {
    let mut supervisor = Supervisor::new();
    let id = supervisor.start_with(sleeping_spec())?;

    supervisor.stop(id).await?;

    assert!(supervisor.stop(id).await.is_err());
    Ok(())
}

#[tokio::test]
async fn reaps_exited_instances() -> Result<(), Box<dyn Error>> {
    let spec = ProcessSpec {
        command: PathBuf::from("sh"),
        args: vec!["-c".to_string(), "exit 7".to_string()],
        working_directory: None,
        environment: BTreeMap::new(),
        shutdown_timeout: Duration::from_secs(5),
    };
    let mut supervisor = Supervisor::new();
    let id = supervisor.start_with(spec)?;

    let mut exited = Vec::new();
    for _ in 0..50 {
        exited = supervisor.poll()?;
        if !exited.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    assert_eq!(exited.len(), 1);
    assert_eq!(exited[0].id, id);
    assert_eq!(exited[0].status.code(), Some(7));
    assert!(supervisor.running().is_empty());
    Ok(())
}
