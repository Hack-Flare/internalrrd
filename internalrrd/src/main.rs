use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use internalrrd::config::{Config, ProxyProtocol};
use internalrrd::deployment::Deployment;
use internalrrd::management::{self, ManagementContext};
use internalrrd::ports::PortAllocator;
use internalrrd::proxy::{ListenerSpec, Proxy, Transport};
use internalrrd::release::ReleaseManager;
use internalrrd::supervisor::Supervisor;
use internalrrd::trust::TrustStore;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "internalrrd",
    version,
    about = "Supervise application processes inside a persistent container"
)]
struct Cli {
    /// Path to the configuration file.
    #[arg(short, long, default_value = "/etc/internalrrd/config.toml")]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error = %error, "internalrrd stopped with an error");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let config = Config::load(&cli.config)?;
    tracing::info!(
        application = %config.application.name,
        management_port = config.management.port,
        "loaded configuration"
    );

    let listener = TcpListener::bind(("0.0.0.0", config.management.port)).await?;
    let management_address = listener.local_addr()?;

    let trust = TrustStore::from_config(&config.trust)?;
    if trust.is_empty() {
        tracing::warn!(
            "no trusted deployment keys are configured, all deployments will be rejected"
        );
    }

    let releases = ReleaseManager::new(
        &config.application.release_directory,
        config.application.max_artifact_bytes,
    )?;

    let proxy = start_proxy(&config).await?;
    let reserved_ports = std::iter::once(config.management.port).chain(
        config
            .proxy
            .listeners
            .iter()
            .map(|listener| listener.port)
            .filter(|port| *port != 0),
    );
    let ports = PortAllocator::new(config.proxy.ports.as_ref(), reserved_ports);
    let supervisor = Arc::new(Mutex::new(Supervisor::new()));

    let deployment = Arc::new(Deployment::new(
        config.application.clone(),
        trust,
        releases,
        proxy,
        ports,
        supervisor.clone(),
    ));
    deployment.start_initial().await?;
    tracing::info!(
        instances = supervisor.lock().await.running().len(),
        version = ?deployment.current_version().await,
        "application startup complete"
    );

    let context = ManagementContext::new(
        config.application.name.clone(),
        config.application.working_directory.clone(),
        supervisor.clone(),
    )
    .with_deployment(deployment.clone());

    let management_task = tokio::spawn(async move {
        if let Err(error) = management::serve(listener, context).await {
            tracing::error!(error = %error, "management listener stopped");
        }
    });
    tracing::info!(%management_address, "management listener started");

    wait_for_shutdown().await?;

    management_task.abort();
    tracing::info!(
        instances = supervisor.lock().await.running().len(),
        "stopping application instances"
    );
    supervisor.lock().await.stop_all().await?;
    tracing::info!("internalrrd stopped");
    Ok(())
}

async fn start_proxy(config: &Config) -> Result<Option<Arc<Proxy>>, Box<dyn Error>> {
    if config.proxy.listeners.is_empty() {
        return Ok(None);
    }

    let specs: Vec<ListenerSpec> = config
        .proxy
        .listeners
        .iter()
        .map(|listener| ListenerSpec {
            transport: match listener.protocol {
                ProxyProtocol::Tcp => Transport::Tcp,
                ProxyProtocol::Udp => Transport::Udp,
            },
            bind: listener.bind,
            port: listener.port,
        })
        .collect();
    let timeout = Duration::from_secs(config.proxy.udp_session_timeout_secs);
    let proxy = Proxy::start(&specs, timeout).await?;

    for address in proxy.addresses() {
        tracing::info!(%address, "proxy listener started");
    }

    Ok(Some(Arc::new(proxy)))
}

async fn wait_for_shutdown() -> Result<(), io::Error> {
    #[cfg(unix)]
    {
        let mut terminate = signal::unix::signal(signal::unix::SignalKind::terminate())?;
        tokio::select! {
            result = signal::ctrl_c() => result,
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        signal::ctrl_c().await
    }
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("internalrrd=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
