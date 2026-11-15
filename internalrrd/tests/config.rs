use std::collections::BTreeMap;
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

use internalrrd::config::{
    ApplicationConfig, ApplicationHealthConfig, ApplicationPortConfig, Config,
    DEFAULT_HEALTH_INTERVAL_SECS, DEFAULT_HEALTH_TIMEOUT_SECS, DEFAULT_INSTANCES,
    DEFAULT_MANAGEMENT_PORT, DEFAULT_MAX_ARTIFACT_BYTES, DEFAULT_PORT_NAME,
    DEFAULT_RELEASE_DIRECTORY, DEFAULT_STARTUP_GRACE_SECS, DEFAULT_UDP_SESSION_TIMEOUT_SECS,
    ManagementConfig, PortRangeConfig, ProxyConfig, ProxyListenerConfig, ProxyProtocol,
    TrustConfig, TrustedKeyConfig,
};

fn minimal_config() -> Config {
    Config {
        management: ManagementConfig::default(),
        application: ApplicationConfig {
            name: "example".to_string(),
            command: PathBuf::from("/usr/bin/example"),
            args: Vec::new(),
            working_directory: None,
            environment: BTreeMap::new(),
            shutdown_timeout_secs: 30,
            instances: DEFAULT_INSTANCES,
            startup_grace_secs: DEFAULT_STARTUP_GRACE_SECS,
            health: ApplicationHealthConfig::default(),
            release_directory: PathBuf::from(DEFAULT_RELEASE_DIRECTORY),
            max_artifact_bytes: DEFAULT_MAX_ARTIFACT_BYTES,
            ports: Vec::new(),
        },
        trust: TrustConfig::default(),
        proxy: ProxyConfig::default(),
    }
}

fn default_bind_address() -> IpAddr {
    IpAddr::V4(Ipv4Addr::UNSPECIFIED)
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
    assert!(config.application.health.enabled);
    assert_eq!(
        config.application.health.interval_secs,
        DEFAULT_HEALTH_INTERVAL_SECS
    );
    assert_eq!(
        config.application.health.timeout_secs,
        DEFAULT_HEALTH_TIMEOUT_SECS
    );
    assert_eq!(
        config.application.release_directory,
        PathBuf::from(DEFAULT_RELEASE_DIRECTORY)
    );
    assert!(config.trust.keys.is_empty());
    assert!(config.proxy.listeners.is_empty());
    assert!(config.proxy.ports.is_none());
    assert!(config.application.ports.is_empty());
    let port_specs = config.application.port_specs();
    assert_eq!(port_specs.len(), 1);
    assert_eq!(port_specs[0].0, DEFAULT_PORT_NAME);
    assert_eq!(
        port_specs[0].1,
        &[ProxyProtocol::Tcp, ProxyProtocol::Udp][..]
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
        health.enabled = false
        health.interval_secs = 3
        health.timeout_secs = 11
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
    assert!(!config.application.health.enabled);
    assert_eq!(config.application.health.interval_secs, 3);
    assert_eq!(config.application.health.timeout_secs, 11);
    assert_eq!(config.application.health_interval().as_secs(), 3);
    assert_eq!(config.application.health_timeout().as_secs(), 11);
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
fn rejects_zero_health_interval() {
    let mut config = minimal_config();
    config.application.health.interval_secs = 0;

    assert!(config.validate().is_err());
}

#[test]
fn rejects_zero_health_timeout() {
    let mut config = minimal_config();
    config.application.health.timeout_secs = 0;

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
        backend: DEFAULT_PORT_NAME.to_string(),
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
