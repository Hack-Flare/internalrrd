use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use internalrrd::config::Config;
use internalrrd::management::{self, ManagementContext};
use internalrrd::process::ProcessSpec;
use internalrrd::supervisor::Supervisor;
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

    let supervisor = Arc::new(Mutex::new(Supervisor::new(ProcessSpec::from(
        &config.application,
    ))));
    let id = supervisor.lock().await.start()?;
    tracing::info!(instance = %id, "application instance started");

    let context = ManagementContext::new(
        config.application.name.clone(),
        config.application.working_directory.clone(),
        supervisor.clone(),
    );
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
