use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;

use console::config::{Config, DEFAULT_TIMEOUT_SECS};

fn minimal_config() -> Config {
    Config {
        application: console::config::ApplicationConfig {
            name: "example".to_string(),
        },
        signing: console::config::SigningConfig {
            key_id: "release-2026".to_string(),
            private_key: "00".repeat(32),
        },
        network: console::config::NetworkConfig::default(),
        servers: vec![console::config::ServerConfig {
            name: "web-1".to_string(),
            address: "127.0.0.1:54".parse().expect("valid address"),
        }],
    }
}

#[test]
fn parses_the_example_configuration() -> Result<(), Box<dyn Error>> {
    let config: Config = toml::from_str(include_str!("../config.example.toml"))?;
    config.validate()?;

    assert_eq!(config.application.name, "example");
    assert_eq!(config.signing.key_id, "release-2026");
    assert_eq!(config.network.timeout_secs, DEFAULT_TIMEOUT_SECS);
    assert_eq!(config.servers.len(), 2);
    assert_eq!(config.servers[0].name, "web-1");
    assert_eq!(config.servers[1].name, "web-2");
    assert_eq!(config.timeout().as_secs(), DEFAULT_TIMEOUT_SECS);
    Ok(())
}

#[test]
fn applies_network_defaults() -> Result<(), Box<dyn Error>> {
    let config: Config = toml::from_str(
        r#"
        [application]
        name = "example"

        [signing]
        key_id = "release-2026"
        private_key = "0000000000000000000000000000000000000000000000000000000000000000"

        [[servers]]
        name = "web-1"
        address = "127.0.0.1:54"
        "#,
    )?;

    assert_eq!(config.network.timeout_secs, DEFAULT_TIMEOUT_SECS);
    assert_eq!(
        config.servers[0].address,
        "127.0.0.1:54".parse::<SocketAddr>()?
    );
    Ok(())
}

#[test]
fn rejects_empty_application_name() {
    let mut config = minimal_config();
    config.application.name = "  ".to_string();

    assert!(config.validate().is_err());
}

#[test]
fn rejects_empty_key_id() {
    let mut config = minimal_config();
    config.signing.key_id = "  ".to_string();

    assert!(config.validate().is_err());
}

#[test]
fn rejects_invalid_private_key_hex() {
    let mut config = minimal_config();
    config.signing.private_key = "not-hex".to_string();

    assert!(config.validate().is_err());
}

#[test]
fn rejects_private_keys_with_the_wrong_length() {
    let mut config = minimal_config();
    config.signing.private_key = "00".repeat(31);

    assert!(config.validate().is_err());
}

#[test]
fn rejects_zero_network_timeout() {
    let mut config = minimal_config();
    config.network.timeout_secs = 0;

    assert!(config.validate().is_err());
}

#[test]
fn rejects_missing_servers() {
    let mut config = minimal_config();
    config.servers.clear();

    assert!(config.validate().is_err());
}

#[test]
fn rejects_duplicate_server_names() {
    let mut config = minimal_config();
    let server = config.servers[0].clone();
    config.servers.push(server);

    assert!(config.validate().is_err());
}

#[test]
fn rejects_duplicate_server_addresses() {
    let mut config = minimal_config();
    let mut server = config.servers[0].clone();
    server.name = "web-2".to_string();
    config.servers.push(server);

    assert!(config.validate().is_err());
}

#[test]
fn rejects_unknown_fields() {
    let result: Result<Config, _> = toml::from_str(
        r#"
        [application]
        name = "example"
        unknown = true

        [signing]
        key_id = "release-2026"
        private_key = "0000000000000000000000000000000000000000000000000000000000000000"

        [[servers]]
        name = "web-1"
        address = "127.0.0.1:54"
        "#,
    );

    assert!(result.is_err());
}

#[test]
fn loads_a_configuration_file() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let path = temporary.path().join("console.toml");
    std::fs::write(&path, include_str!("../config.example.toml"))?;

    let config = Config::load(&path)?;

    assert_eq!(config.servers.len(), 2);
    assert_eq!(
        config.servers[0].address,
        "127.0.0.1:54".parse::<SocketAddr>()?
    );
    Ok(())
}

#[test]
fn reports_unreadable_configuration_files() {
    let result = Config::load(PathBuf::from("/nonexistent/console.toml"));

    assert!(result.is_err());
}
