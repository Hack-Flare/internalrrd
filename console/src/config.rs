use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

/// The default management protocol timeout for console requests.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub application: ApplicationConfig,
    pub signing: SigningConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    /// Servers that receive deployments and answer status queries.
    #[serde(default)]
    pub servers: Vec<ServerConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationConfig {
    /// The application name that deployments target.
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SigningConfig {
    /// The identifier of the key in every server's trust store.
    pub key_id: String,
    /// The 32-byte Ed25519 private key as hexadecimal.
    pub private_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// How long a connection or response may take.
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            timeout_secs: default_timeout_secs(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// A unique name used to select the server on the command line.
    pub name: String,
    /// The management protocol address.
    pub address: SocketAddr,
}

impl Config {
    /// Load and validate a configuration file.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let contents = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Config = toml::from_str(&contents).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    /// Validate cross-field configuration rules.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.application.name.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "application.name must not be empty".to_string(),
            ));
        }
        if self.signing.key_id.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "signing.key_id must not be empty".to_string(),
            ));
        }
        let private_key = hex::decode(self.signing.private_key.trim()).map_err(|error| {
            ConfigError::Invalid(format!(
                "signing.private_key is not valid hexadecimal: {error}"
            ))
        })?;
        if private_key.len() != 32 {
            return Err(ConfigError::Invalid(format!(
                "signing.private_key must be 32 bytes, got {}",
                private_key.len()
            )));
        }
        if self.network.timeout_secs == 0 {
            return Err(ConfigError::Invalid(
                "network.timeout_secs must be greater than zero".to_string(),
            ));
        }
        if self.servers.is_empty() {
            return Err(ConfigError::Invalid(
                "at least one server must be configured".to_string(),
            ));
        }

        let mut names = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        for server in &self.servers {
            if server.name.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "servers[].name must not be empty".to_string(),
                ));
            }
            if !names.insert(server.name.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "server '{}' is defined more than once",
                    server.name
                )));
            }
            if !addresses.insert(server.address) {
                return Err(ConfigError::Invalid(format!(
                    "server address {} is defined more than once",
                    server.address
                )));
            }
        }

        Ok(())
    }

    /// The request timeout as a duration.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.network.timeout_secs)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read configuration from {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("failed to parse configuration from {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("invalid configuration: {0}")]
    Invalid(String),
}
