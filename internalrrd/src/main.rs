use std::error::Error;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use internalrrd::config::Config;
use internalrrd::process::ProcessSpec;
use internalrrd::supervisor::Supervisor;
use tokio::signal;
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

    let mut supervisor = Supervisor::new(ProcessSpec::from(&config.application));
    let id = supervisor.start()?;
    tracing::info!(instance = %id, "application instance started");

    wait_for_shutdown().await?;

    tracing::info!(
        instances = supervisor.running().len(),
        "stopping application instances"
    );
    supervisor.stop_all().await?;
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
