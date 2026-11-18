use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::{Args, Parser, Subcommand};
use console::client::Client;
use console::config::{Config, ServerConfig};
use console::signing::Signer;
use protocol::{Artifact, ComponentMetrics, Deployment, MetricsReport, StatusReport};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "console",
    version,
    about = "Sign deployments and manage InternalRRD servers"
)]
struct Cli {
    /// Path to the configuration file.
    #[arg(short, long, default_value = "/etc/internalrrd/console.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Verify connectivity and print the server handshake.
    Hello {
        /// Restrict the request to one configured server.
        #[arg(long)]
        server: Option<String>,
    },
    /// Print the deployed version and instances.
    Status {
        /// Restrict the request to one configured server.
        #[arg(long)]
        server: Option<String>,
    },
    /// Print a metrics snapshot.
    Metrics {
        /// Restrict the request to one configured server.
        #[arg(long)]
        server: Option<String>,
        /// Restrict instance metrics to one instance identifier.
        #[arg(long)]
        instance: Option<u64>,
    },
    /// Sign and send a deployment.
    Deploy(DeployArgs),
}

#[derive(Debug, Args)]
struct DeployArgs {
    /// The release version to deploy.
    #[arg(long)]
    version: String,
    /// The URL the servers download the artifact from.
    #[arg(long)]
    artifact: String,
    /// The artifact SHA-256 digest as hexadecimal.
    #[arg(long)]
    sha256: String,
    /// The artifact size in bytes.
    #[arg(long)]
    size: u64,
    /// The deployment issue time in Unix seconds. Defaults to now.
    #[arg(long)]
    issued_at: Option<u64>,
    /// Restrict the deployment to one configured server.
    #[arg(long)]
    server: Option<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    match run(Cli::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let config = Config::load(&cli.config)?;

    match cli.command {
        Command::Hello { server } => run_hello(&config, server.as_deref()).await,
        Command::Status { server } => run_status(&config, server.as_deref()).await,
        Command::Metrics { server, instance } => {
            run_metrics(&config, server.as_deref(), instance).await
        }
        Command::Deploy(arguments) => run_deploy(&config, &arguments).await,
    }
}

async fn run_hello(config: &Config, name: Option<&str>) -> Result<(), Box<dyn Error>> {
    let mut failed = false;

    for server in select_servers(config, name)? {
        match Client::connect(server.address, &config.application.name, config.timeout()).await {
            Ok(client) => {
                let hello = client.hello();
                let capabilities: Vec<u16> =
                    hello.capabilities.iter().map(|value| value.raw()).collect();
                println!(
                    "{}: application {} version {} capabilities {:?}",
                    server.name,
                    hello.application,
                    hello.deployed_version.as_deref().unwrap_or("none"),
                    capabilities
                );
            }
            Err(error) => {
                eprintln!("{}: {error}", server.name);
                failed = true;
            }
        }
    }

    fail_if_any(failed)
}

async fn run_status(config: &Config, name: Option<&str>) -> Result<(), Box<dyn Error>> {
    let mut failed = false;

    for server in select_servers(config, name)? {
        match status_from(config, server).await {
            Ok(status) => print_status(&server.name, &status),
            Err(error) => {
                eprintln!("{}: {error}", server.name);
                failed = true;
            }
        }
    }

    fail_if_any(failed)
}

async fn run_metrics(
    config: &Config,
    name: Option<&str>,
    instance: Option<u64>,
) -> Result<(), Box<dyn Error>> {
    let mut failed = false;

    for server in select_servers(config, name)? {
        match metrics_from(config, server, instance).await {
            Ok(report) => print_metrics(&server.name, &report),
            Err(error) => {
                eprintln!("{}: {error}", server.name);
                failed = true;
            }
        }
    }

    fail_if_any(failed)
}

async fn run_deploy(config: &Config, arguments: &DeployArgs) -> Result<(), Box<dyn Error>> {
    if arguments.version.trim().is_empty() {
        return Err("version must not be empty".into());
    }

    let digest = hex::decode(arguments.sha256.trim())
        .map_err(|error| format!("invalid sha256 digest: {error}"))?;
    let sha256: [u8; 32] = digest.try_into().map_err(|digest: Vec<u8>| {
        format!("sha256 digest must be 32 bytes, got {}", digest.len())
    })?;
    let issued_at_unix = match arguments.issued_at {
        Some(issued_at_unix) => issued_at_unix,
        None => unix_timestamp()?,
    };

    let deployment = Deployment {
        application: config.application.name.clone(),
        version: arguments.version.clone(),
        artifact: Artifact {
            url: arguments.artifact.clone(),
            sha256,
            size_bytes: arguments.size,
        },
        issued_at_unix,
    };
    let signed = Signer::from_config(&config.signing)?.sign(deployment)?;

    for server in select_servers(config, arguments.server.as_deref())? {
        match deploy_to(config, server, &signed).await {
            Ok(accepted) => println!("{}: deployed version {}", server.name, accepted.version),
            Err(error) => {
                eprintln!("{}: {error}", server.name);
                return Err(
                    format!("deployment stopped because server '{}' failed", server.name).into(),
                );
            }
        }
    }

    Ok(())
}

async fn status_from(
    config: &Config,
    server: &ServerConfig,
) -> Result<StatusReport, Box<dyn Error>> {
    let mut client =
        Client::connect(server.address, &config.application.name, config.timeout()).await?;
    Ok(client.status().await?)
}

async fn metrics_from(
    config: &Config,
    server: &ServerConfig,
    instance: Option<u64>,
) -> Result<MetricsReport, Box<dyn Error>> {
    let mut client =
        Client::connect(server.address, &config.application.name, config.timeout()).await?;
    Ok(client.metrics(instance).await?)
}

async fn deploy_to(
    config: &Config,
    server: &ServerConfig,
    signed: &protocol::SignedDeployment,
) -> Result<protocol::DeploymentAccepted, Box<dyn Error>> {
    let mut client =
        Client::connect(server.address, &config.application.name, config.timeout()).await?;
    Ok(client.deploy(signed.clone()).await?)
}

fn select_servers<'a>(
    config: &'a Config,
    name: Option<&str>,
) -> Result<Vec<&'a ServerConfig>, Box<dyn Error>> {
    match name {
        Some(name) => {
            let server = config
                .servers
                .iter()
                .find(|server| server.name == name)
                .ok_or_else(|| format!("unknown server '{name}'"))?;
            Ok(vec![server])
        }
        None => Ok(config.servers.iter().collect()),
    }
}

fn fail_if_any(failed: bool) -> Result<(), Box<dyn Error>> {
    if failed {
        return Err("one or more servers failed".into());
    }
    Ok(())
}

fn unix_timestamp() -> Result<u64, Box<dyn Error>> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn print_status(server: &str, status: &StatusReport) {
    match &status.deployed_version {
        Some(version) => println!("{server}: {} version {version}", status.application),
        None => println!("{server}: {} has no deployed version", status.application),
    }

    for instance in &status.instances {
        println!(
            "  instance {} pid {} {} uptime {}",
            instance.id,
            instance.pid,
            health_label(instance.healthy),
            format_duration(instance.uptime_secs)
        );
    }
}

fn print_metrics(server: &str, report: &MetricsReport) {
    println!("{server}:");

    let Some(application) = &report.application else {
        println!("  no application metrics");
        return;
    };

    match &application.deployed_version {
        Some(version) => println!("  application {} version {version}", application.name),
        None => println!("  application {} has no deployed version", application.name),
    }
    print_component("  internalrrd", &report.internalrrd);
    print_component("  aggregate", &application.aggregate);

    for instance in &application.instances {
        println!(
            "  instance {} pid {} {} uptime {}",
            instance.id,
            instance.pid,
            health_label(instance.healthy),
            format_duration(instance.uptime_secs)
        );
        print_component("    metrics", &instance.metrics);
    }
}

fn print_component(prefix: &str, component: &ComponentMetrics) {
    let resources = &component.resources;
    print!(
        "{prefix}: cpu {:.2}% memory {} storage {}",
        f64::from(resources.cpu_basis_points) / 100.0,
        format_bytes(resources.memory_used_bytes),
        format_bytes(resources.storage_used_bytes)
    );
    if let Some(limit) = resources.storage_limit_bytes {
        print!(" / {}", format_bytes(limit));
    }
    println!(" requests {}", component.traffic.total_requests);
}

fn health_label(healthy: bool) -> &'static str {
    if healthy { "healthy" } else { "unhealthy" }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes / KIB)
    } else {
        format!("{} B", bytes as u64)
    }
}

fn format_duration(seconds: u64) -> String {
    if seconds >= 3600 {
        format!("{}h{}m", seconds / 3600, (seconds % 3600) / 60)
    } else if seconds >= 60 {
        format!("{}m{}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("console=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
