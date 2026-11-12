use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

/// The default port used for the InternalRRD management protocol.
pub const DEFAULT_MANAGEMENT_PORT: u16 = 54;

fn default_management_port() -> u16 {
    DEFAULT_MANAGEMENT_PORT
}

fn default_shutdown_timeout_secs() -> u64 {
    30
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub management: ManagementConfig,
    pub application: ApplicationConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagementConfig {
    /// The port that the management protocol listens on.
    #[serde(default = "default_management_port")]
    pub port: u16,
}

impl Default for ManagementConfig {
    fn default() -> Self {
        Self {
            port: default_management_port(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationConfig {
    /// A human readable name for the application.
    pub name: String,
    /// The executable to run for each application instance.
    pub command: PathBuf,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// The directory the application instance runs in.
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    /// Extra environment variables passed to the application instance.
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    /// How long an instance has to shut down before it is killed.
    #[serde(default = "default_shutdown_timeout_secs")]
    pub shutdown_timeout_secs: u64,
}

impl ApplicationConfig {
    /// The graceful shutdown timeout as a duration.
    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_secs(self.shutdown_timeout_secs)
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

    fn validate(&self) -> Result<(), ConfigError> {
        if self.application.name.trim().is_empty() {
            return Err(ConfigError::Invalid(
                "application.name must not be empty".to_string(),
            ));
        }
        if self.application.command.as_os_str().is_empty() {
            return Err(ConfigError::Invalid(
                "application.command must not be empty".to_string(),
            ));
        }
        if self.application.shutdown_timeout_secs == 0 {
            return Err(ConfigError::Invalid(
                "application.shutdown_timeout_secs must be greater than zero".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    fn minimal_config() -> Config {
        Config {
            management: ManagementConfig::default(),
            application: ApplicationConfig {
                name: "example".to_string(),
                command: PathBuf::from("/usr/bin/example"),
                args: Vec::new(),
                working_directory: None,
                environment: BTreeMap::new(),
                shutdown_timeout_secs: default_shutdown_timeout_secs(),
            },
        }
    }

    #[test]
    fn parses_defaults() -> Result<(), Box<dyn Error>> {
        let config: Config = toml::from_str(
            r#"
            [application]
            name = "example"
            command = "/usr/bin/example"
            "#,
        )?;

        assert_eq!(config.management.port, DEFAULT_MANAGEMENT_PORT);
        assert!(config.application.args.is_empty());
        assert!(config.application.working_directory.is_none());
        assert!(config.application.environment.is_empty());
        assert_eq!(config.application.shutdown_timeout_secs, 30);
        Ok(())
    }

    #[test]
    fn parses_application_settings() -> Result<(), Box<dyn Error>> {
        let config: Config = toml::from_str(
            r#"
            [management]
            port = 1054

            [application]
            name = "example"
            command = "/usr/bin/example"
            args = ["--serve"]
            working_directory = "/srv/example"
            shutdown_timeout_secs = 5

            [application.environment]
            RUST_LOG = "info"
            "#,
        )?;

        assert_eq!(config.management.port, 1054);
        assert_eq!(config.application.args, vec!["--serve".to_string()]);
        assert_eq!(
            config.application.working_directory,
            Some(PathBuf::from("/srv/example"))
        );
        assert_eq!(
            config.application.environment.get("RUST_LOG"),
            Some(&"info".to_string())
        );
        assert_eq!(config.application.shutdown_timeout().as_secs(), 5);
        Ok(())
    }

    #[test]
    fn rejects_empty_application_name() {
        let mut config = minimal_config();
        config.application.name = "  ".to_string();

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_empty_command() {
        let mut config = minimal_config();
        config.application.command = PathBuf::new();

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_zero_shutdown_timeout() {
        let mut config = minimal_config();
        config.application.shutdown_timeout_secs = 0;

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let result: Result<Config, _> = toml::from_str(
            r#"
            [application]
            name = "example"
            command = "/usr/bin/example"
            unknown = true
            "#,
        );

        assert!(result.is_err());
    }
}
