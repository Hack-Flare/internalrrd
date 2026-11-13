use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

pub use protocol::DEFAULT_MANAGEMENT_PORT;

/// The default number of application instances for a release.
pub const DEFAULT_INSTANCES: u8 = 1;
/// How long new instances must stay alive before traffic is switched to them.
pub const DEFAULT_STARTUP_GRACE_SECS: u64 = 2;
/// The default directory where release artifacts are extracted.
pub const DEFAULT_RELEASE_DIRECTORY: &str = "/var/lib/internalrrd/releases";
/// The default maximum artifact size that will be downloaded.
pub const DEFAULT_MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
/// The default idle timeout for UDP proxy sessions.
pub const DEFAULT_UDP_SESSION_TIMEOUT_SECS: u64 = 30;
/// The name of the implicit application port used when none are configured.
pub const DEFAULT_PORT_NAME: &str = "default";

/// The protocols that the implicit application port may serve.
const DEFAULT_PORT_PROTOCOLS: &[ProxyProtocol] = &[ProxyProtocol::Tcp, ProxyProtocol::Udp];

fn default_management_port() -> u16 {
    DEFAULT_MANAGEMENT_PORT
}

fn default_shutdown_timeout_secs() -> u64 {
    30
}

fn default_instances() -> u8 {
    DEFAULT_INSTANCES
}

fn default_startup_grace_secs() -> u64 {
    DEFAULT_STARTUP_GRACE_SECS
}

fn default_release_directory() -> PathBuf {
    PathBuf::from(DEFAULT_RELEASE_DIRECTORY)
}

fn default_max_artifact_bytes() -> u64 {
    DEFAULT_MAX_ARTIFACT_BYTES
}

fn default_udp_session_timeout_secs() -> u64 {
    DEFAULT_UDP_SESSION_TIMEOUT_SECS
}

fn default_bind_address() -> IpAddr {
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
}

fn default_signature_algorithm() -> u8 {
    1
}

fn default_backend_name() -> String {
    DEFAULT_PORT_NAME.to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub management: ManagementConfig,
    pub application: ApplicationConfig,
    #[serde(default)]
    pub trust: TrustConfig,
    #[serde(default)]
    pub proxy: ProxyConfig,
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
    /// The entrypoint to run for each instance.
    ///
    /// This is a template that may use `{release_dir}`, `{port}`, and `{listen_address}`.
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
    /// The number of application instances started for each release.
    #[serde(default = "default_instances")]
    pub instances: u8,
    /// How long a new instance must stay alive before it receives traffic.
    #[serde(default = "default_startup_grace_secs")]
    pub startup_grace_secs: u64,
    /// The directory where release artifacts are extracted.
    #[serde(default = "default_release_directory")]
    pub release_directory: PathBuf,
    /// The maximum artifact size that will be downloaded.
    #[serde(default = "default_max_artifact_bytes")]
    pub max_artifact_bytes: u64,
    /// Named private ports that are assigned to every application instance.
    ///
    /// When empty, a single port named `default` is allocated per instance.
    #[serde(default)]
    pub ports: Vec<ApplicationPortConfig>,
}

/// One named private port that application instances listen on.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationPortConfig {
    /// The name used in template placeholders and proxy listener backends.
    pub name: String,
    /// The transports the application port serves.
    pub protocols: Vec<ProxyProtocol>,
}

impl ApplicationConfig {
    /// The configured ports, or the implicit default port when none are configured.
    pub fn port_specs(&self) -> Vec<(&str, &[ProxyProtocol])> {
        if self.ports.is_empty() {
            return vec![(DEFAULT_PORT_NAME, DEFAULT_PORT_PROTOCOLS)];
        }

        self.ports
            .iter()
            .map(|port| (port.name.as_str(), port.protocols.as_slice()))
            .collect()
    }

    /// The protocols declared for an application port name.
    pub fn port_protocols(&self, name: &str) -> Option<&[ProxyProtocol]> {
        if self.ports.is_empty() {
            return (name == DEFAULT_PORT_NAME).then_some(DEFAULT_PORT_PROTOCOLS);
        }

        self.ports
            .iter()
            .find(|port| port.name == name)
            .map(|port| port.protocols.as_slice())
    }

    /// The graceful shutdown timeout as a duration.
    pub fn shutdown_timeout(&self) -> Duration {
        Duration::from_secs(self.shutdown_timeout_secs)
    }

    /// The startup grace period as a duration.
    pub fn startup_grace(&self) -> Duration {
        Duration::from_secs(self.startup_grace_secs)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustConfig {
    /// Release signing keys that deployments are verified against.
    #[serde(default)]
    pub keys: Vec<TrustedKeyConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedKeyConfig {
    /// A stable identifier that a signed deployment references.
    pub id: String,
    /// The signature algorithm identifier. `1` is Ed25519.
    #[serde(default = "default_signature_algorithm")]
    pub algorithm: u8,
    /// The public key encoded as lowercase or uppercase hexadecimal.
    pub public_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyConfig {
    /// Public L4 listeners that forward to application instances.
    #[serde(default)]
    pub listeners: Vec<ProxyListenerConfig>,
    /// The private port range used by application instances.
    ///
    /// When absent, ephemeral ports are allocated instead.
    #[serde(default)]
    pub ports: Option<PortRangeConfig>,
    /// How long an idle UDP client session is kept open.
    #[serde(default = "default_udp_session_timeout_secs")]
    pub udp_session_timeout_secs: u64,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listeners: Vec::new(),
            ports: None,
            udp_session_timeout_secs: default_udp_session_timeout_secs(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyProtocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyListenerConfig {
    /// The transport this listener accepts.
    pub protocol: ProxyProtocol,
    #[serde(default = "default_bind_address")]
    pub bind: IpAddr,
    /// The public port this listener binds.
    pub port: u16,
    /// The application port name that supplies backends for this listener.
    #[serde(default = "default_backend_name")]
    pub backend: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortRangeConfig {
    pub start: u16,
    pub end: u16,
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
        if self.application.instances == 0 {
            return Err(ConfigError::Invalid(
                "application.instances must be greater than zero".to_string(),
            ));
        }
        if self.application.release_directory.as_os_str().is_empty() {
            return Err(ConfigError::Invalid(
                "application.release_directory must not be empty".to_string(),
            ));
        }
        if self.application.max_artifact_bytes == 0 {
            return Err(ConfigError::Invalid(
                "application.max_artifact_bytes must be greater than zero".to_string(),
            ));
        }

        self.validate_application_ports()?;
        self.validate_trust()?;
        self.validate_proxy()
    }

    fn validate_application_ports(&self) -> Result<(), ConfigError> {
        let mut names = BTreeSet::new();
        for port in &self.application.ports {
            if port.name.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "application.ports[].name must not be empty".to_string(),
                ));
            }
            if !names.insert(port.name.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "application port '{}' is defined more than once",
                    port.name
                )));
            }
            if port.protocols.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "application port '{}' must declare at least one protocol",
                    port.name
                )));
            }

            let mut protocols = BTreeSet::new();
            for protocol in &port.protocols {
                if !protocols.insert(*protocol) {
                    return Err(ConfigError::Invalid(format!(
                        "application port '{}' declares the {:?} protocol more than once",
                        port.name, protocol
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_trust(&self) -> Result<(), ConfigError> {
        let mut ids = BTreeSet::new();
        for key in &self.trust.keys {
            if key.id.trim().is_empty() {
                return Err(ConfigError::Invalid(
                    "trust.keys[].id must not be empty".to_string(),
                ));
            }
            if !ids.insert(key.id.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "trust key '{}' is defined more than once",
                    key.id
                )));
            }
            if key.algorithm != 1 {
                return Err(ConfigError::Invalid(format!(
                    "trust key '{}' uses unsupported signature algorithm {}",
                    key.id, key.algorithm
                )));
            }
            let decoded = hex::decode(key.public_key.trim()).map_err(|error| {
                ConfigError::Invalid(format!(
                    "trust key '{}' has an invalid hexadecimal public key: {error}",
                    key.id
                ))
            })?;
            if decoded.len() != 32 {
                return Err(ConfigError::Invalid(format!(
                    "trust key '{}' public key must be 32 bytes, got {}",
                    key.id,
                    decoded.len()
                )));
            }
        }
        Ok(())
    }

    fn validate_proxy(&self) -> Result<(), ConfigError> {
        if self.proxy.udp_session_timeout_secs == 0 {
            return Err(ConfigError::Invalid(
                "proxy.udp_session_timeout_secs must be greater than zero".to_string(),
            ));
        }

        let mut listeners = BTreeSet::new();
        for listener in &self.proxy.listeners {
            let Some(protocols) = self.application.port_protocols(&listener.backend) else {
                return Err(ConfigError::Invalid(format!(
                    "proxy listener {:?} {}:{} references unknown application port '{}'",
                    listener.protocol, listener.bind, listener.port, listener.backend
                )));
            };
            if !protocols.contains(&listener.protocol) {
                return Err(ConfigError::Invalid(format!(
                    "proxy listener {:?} {}:{} targets application port '{}', which does not declare the {:?} protocol",
                    listener.protocol,
                    listener.bind,
                    listener.port,
                    listener.backend,
                    listener.protocol
                )));
            }

            if listener.port == self.management.port {
                return Err(ConfigError::Invalid(format!(
                    "proxy listener port {} conflicts with the management port",
                    listener.port
                )));
            }
            if listener.port != 0 {
                let key = (listener.protocol, listener.bind, listener.port);
                if !listeners.insert(key) {
                    return Err(ConfigError::Invalid(format!(
                        "proxy listener {:?} {}:{} is defined more than once",
                        listener.protocol, listener.bind, listener.port
                    )));
                }
            }
        }

        if let Some(range) = &self.proxy.ports {
            if range.start == 0 {
                return Err(ConfigError::Invalid(
                    "proxy.ports.start must be greater than zero".to_string(),
                ));
            }
            if range.start > range.end {
                return Err(ConfigError::Invalid(
                    "proxy.ports.start must not be greater than proxy.ports.end".to_string(),
                ));
            }
            if (range.start..=range.end).contains(&self.management.port) {
                return Err(ConfigError::Invalid(format!(
                    "proxy.ports range includes the management port {}",
                    self.management.port
                )));
            }
            for listener in &self.proxy.listeners {
                if listener.port != 0 && (range.start..=range.end).contains(&listener.port) {
                    return Err(ConfigError::Invalid(format!(
                        "proxy.ports range includes listener port {}",
                        listener.port
                    )));
                }
            }
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
                instances: default_instances(),
                startup_grace_secs: default_startup_grace_secs(),
                release_directory: default_release_directory(),
                max_artifact_bytes: default_max_artifact_bytes(),
                ports: Vec::new(),
            },
            trust: TrustConfig::default(),
            proxy: ProxyConfig::default(),
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
        assert_eq!(config.application.instances, DEFAULT_INSTANCES);
        assert_eq!(
            config.application.startup_grace_secs,
            DEFAULT_STARTUP_GRACE_SECS
        );
        assert_eq!(
            config.application.release_directory,
            default_release_directory()
        );
        assert!(config.trust.keys.is_empty());
        assert!(config.proxy.listeners.is_empty());
        assert!(config.proxy.ports.is_none());
        assert!(config.application.ports.is_empty());
        assert_eq!(
            config.application.port_specs(),
            vec![(DEFAULT_PORT_NAME, DEFAULT_PORT_PROTOCOLS)]
        );
        assert_eq!(
            config.proxy.udp_session_timeout_secs,
            DEFAULT_UDP_SESSION_TIMEOUT_SECS
        );
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
            command = "{release_dir}/bin/example"
            args = ["--serve", "--port", "{port}"]
            working_directory = "/srv/example"
            shutdown_timeout_secs = 5
            instances = 3
            startup_grace_secs = 7
            release_directory = "/srv/releases"
            max_artifact_bytes = 1024

            [application.environment]
            LISTEN_ADDR = "{listen_address}"
            "#,
        )?;

        assert_eq!(config.management.port, 1054);
        assert_eq!(
            config.application.args,
            vec![
                "--serve".to_string(),
                "--port".to_string(),
                "{port}".to_string()
            ]
        );
        assert_eq!(
            config.application.working_directory,
            Some(PathBuf::from("/srv/example"))
        );
        assert_eq!(
            config.application.environment.get("LISTEN_ADDR"),
            Some(&"{listen_address}".to_string())
        );
        assert_eq!(config.application.shutdown_timeout().as_secs(), 5);
        assert_eq!(config.application.instances, 3);
        assert_eq!(config.application.startup_grace_secs, 7);
        assert_eq!(
            config.application.release_directory,
            PathBuf::from("/srv/releases")
        );
        assert_eq!(config.application.max_artifact_bytes, 1024);
        Ok(())
    }

    #[test]
    fn parses_trust_and_proxy_settings() -> Result<(), Box<dyn Error>> {
        let config: Config = toml::from_str(
            r#"
            [application]
            name = "example"
            command = "{release_dir}/bin/example"

            [[trust.keys]]
            id = "release-2026"
            public_key = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"

            [[proxy.listeners]]
            protocol = "tcp"
            port = 8080

            [[proxy.listeners]]
            protocol = "udp"
            bind = "127.0.0.1"
            port = 53

            [proxy.ports]
            start = 30000
            end = 30100
            "#,
        )?;

        assert_eq!(config.trust.keys.len(), 1);
        assert_eq!(config.trust.keys[0].algorithm, 1);
        assert_eq!(config.proxy.listeners.len(), 2);
        assert_eq!(config.proxy.listeners[0].protocol, ProxyProtocol::Tcp);
        assert_eq!(config.proxy.listeners[1].protocol, ProxyProtocol::Udp);
        assert_eq!(config.proxy.listeners[0].backend, DEFAULT_PORT_NAME);
        assert_eq!(config.proxy.listeners[1].backend, DEFAULT_PORT_NAME);
        assert_eq!(
            config.proxy.listeners[1].bind,
            "127.0.0.1".parse::<IpAddr>()?
        );
        assert_eq!(
            config
                .proxy
                .ports
                .as_ref()
                .map(|range| (range.start, range.end)),
            Some((30000, 30100))
        );
        config.validate()?;
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
    fn rejects_zero_instances() {
        let mut config = minimal_config();
        config.application.instances = 0;

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_duplicate_trust_keys() {
        let mut config = minimal_config();
        let key = TrustedKeyConfig {
            id: "release".to_string(),
            algorithm: 1,
            public_key: "00".repeat(32),
        };
        config.trust.keys = vec![key.clone(), key];

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_invalid_trust_key_lengths() {
        let mut config = minimal_config();
        config.trust.keys = vec![TrustedKeyConfig {
            id: "release".to_string(),
            algorithm: 1,
            public_key: "00".repeat(31),
        }];

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_proxy_port_conflicts() {
        let mut config = minimal_config();
        config.proxy.listeners = vec![ProxyListenerConfig {
            protocol: ProxyProtocol::Tcp,
            bind: default_bind_address(),
            port: DEFAULT_MANAGEMENT_PORT,
            backend: default_backend_name(),
        }];

        assert!(config.validate().is_err());
    }

    #[test]
    fn parses_named_application_ports() -> Result<(), Box<dyn Error>> {
        let config: Config = toml::from_str(
            r#"
            [application]
            name = "example"
            command = "{release_dir}/bin/example"

            [[application.ports]]
            name = "dns"
            protocols = ["udp", "tcp"]

            [[application.ports]]
            name = "http"
            protocols = ["tcp"]

            [[proxy.listeners]]
            protocol = "udp"
            port = 53
            backend = "dns"

            [[proxy.listeners]]
            protocol = "tcp"
            port = 53
            backend = "dns"

            [[proxy.listeners]]
            protocol = "tcp"
            port = 80
            backend = "http"
            "#,
        )?;
        config.validate()?;

        assert_eq!(config.application.ports.len(), 2);
        assert_eq!(
            config.application.port_protocols("dns"),
            Some(&[ProxyProtocol::Udp, ProxyProtocol::Tcp][..])
        );
        assert_eq!(
            config.application.port_protocols("http"),
            Some(&[ProxyProtocol::Tcp][..])
        );
        assert_eq!(config.proxy.listeners[0].backend, "dns");
        Ok(())
    }

    #[test]
    fn rejects_duplicate_application_port_names() {
        let mut config = minimal_config();
        config.application.ports = vec![
            ApplicationPortConfig {
                name: "dns".to_string(),
                protocols: vec![ProxyProtocol::Udp],
            },
            ApplicationPortConfig {
                name: "dns".to_string(),
                protocols: vec![ProxyProtocol::Tcp],
            },
        ];

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_application_ports_without_protocols() {
        let mut config = minimal_config();
        config.application.ports = vec![ApplicationPortConfig {
            name: "dns".to_string(),
            protocols: Vec::new(),
        }];

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_unknown_listener_backends() {
        let mut config = minimal_config();
        config.proxy.listeners = vec![ProxyListenerConfig {
            protocol: ProxyProtocol::Tcp,
            bind: default_bind_address(),
            port: 8080,
            backend: "missing".to_string(),
        }];

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_listeners_with_an_undeclared_protocol() {
        let mut config = minimal_config();
        config.application.ports = vec![ApplicationPortConfig {
            name: "ntp".to_string(),
            protocols: vec![ProxyProtocol::Udp],
        }];
        config.proxy.listeners = vec![ProxyListenerConfig {
            protocol: ProxyProtocol::Tcp,
            bind: default_bind_address(),
            port: 123,
            backend: "ntp".to_string(),
        }];

        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_port_ranges_containing_the_management_port() {
        let mut config = minimal_config();
        config.proxy.ports = Some(PortRangeConfig {
            start: DEFAULT_MANAGEMENT_PORT - 1,
            end: DEFAULT_MANAGEMENT_PORT + 1,
        });

        assert!(config.validate().is_err());
    }

    #[test]
    fn parses_the_example_configuration() -> Result<(), Box<dyn Error>> {
        let config: Config = toml::from_str(include_str!("../config.example.toml"))?;
        config.validate()?;

        assert_eq!(config.application.name, "example");
        assert_eq!(config.application.instances, 2);
        assert_eq!(config.application.ports.len(), 4);
        assert_eq!(config.proxy.listeners.len(), 5);
        assert_eq!(config.proxy.listeners[0].backend, "ntp");
        Ok(())
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
