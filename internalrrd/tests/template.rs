use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use internalrrd::config::DEFAULT_PORT_NAME;
use internalrrd::template::{TemplateError, TemplateValues, expand};

fn ports() -> BTreeMap<String, u16> {
    BTreeMap::from([
        (DEFAULT_PORT_NAME.to_string(), 30000),
        ("dns".to_string(), 30001),
        ("ntp".to_string(), 30002),
    ])
}

fn values<'a>(
    ports: &'a BTreeMap<String, u16>,
    release_dir: Option<&'a Path>,
) -> TemplateValues<'a> {
    TemplateValues { ports, release_dir }
}

#[test]
fn expands_default_port_placeholders() -> Result<(), TemplateError> {
    let ports = ports();
    let expanded = expand(
        "--port {port} --listen {listen_address} --host {address}",
        &values(&ports, None),
    )?;

    assert_eq!(
        expanded,
        "--port 30000 --listen 127.0.0.1:30000 --host 127.0.0.1"
    );
    Ok(())
}

#[test]
fn expands_named_port_placeholders() -> Result<(), TemplateError> {
    let ports = ports();
    let expanded = expand(
        "--dns {port:dns} --ntp {listen_address:ntp} --dns-host {address:dns}",
        &values(&ports, None),
    )?;

    assert_eq!(
        expanded,
        "--dns 30001 --ntp 127.0.0.1:30002 --dns-host 127.0.0.1"
    );
    Ok(())
}

#[test]
fn expands_release_directories() -> Result<(), TemplateError> {
    let ports = ports();
    let release = PathBuf::from("/srv/releases/1.2.3");
    let expanded = expand("{release_dir}/bin/example", &values(&ports, Some(&release)))?;

    assert_eq!(expanded, "/srv/releases/1.2.3/bin/example");
    Ok(())
}

#[test]
fn rejects_release_placeholders_without_a_release() {
    let ports = ports();

    assert_eq!(
        expand("{release_dir}/bin/example", &values(&ports, None)),
        Err(TemplateError::MissingRelease)
    );
}

#[test]
fn rejects_unknown_port_names() {
    let ports = ports();

    assert_eq!(
        expand("--port {port:missing}", &values(&ports, None)),
        Err(TemplateError::UnknownPort("missing".to_string()))
    );
}

#[test]
fn preserves_unknown_placeholders() -> Result<(), TemplateError> {
    let ports = ports();
    let expanded = expand("{unknown}", &values(&ports, None))?;

    assert_eq!(expanded, "{unknown}");
    Ok(())
}
