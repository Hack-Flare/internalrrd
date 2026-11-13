use std::collections::BTreeMap;
use std::path::Path;

use crate::config::DEFAULT_PORT_NAME;

const LOCAL_ADDRESS: &str = "127.0.0.1";

/// Values substituted into instance command templates.
pub struct TemplateValues<'a> {
    pub ports: &'a BTreeMap<String, u16>,
    pub release_dir: Option<&'a Path>,
}

/// Expand the supported placeholders in a command template.
///
/// Supported placeholders are `{release_dir}`, `{port}`, `{address}`, `{listen_address}`, and their named forms `{port:name}`, `{address:name}`, and `{listen_address:name}`.
/// The unnamed forms resolve to the application port named `default`.
/// Unknown placeholders are left unchanged.
pub fn expand(template: &str, values: &TemplateValues<'_>) -> Result<String, TemplateError> {
    if template.contains("{release_dir}") && values.release_dir.is_none() {
        return Err(TemplateError::MissingRelease);
    }

    let mut expanded = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find('{') {
        expanded.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            expanded.push_str(&rest[start..]);
            return Ok(expanded);
        };

        let placeholder = &after[..end];
        match resolve(placeholder, values)? {
            Some(value) => expanded.push_str(&value),
            None => {
                expanded.push('{');
                expanded.push_str(placeholder);
                expanded.push('}');
            }
        }
        rest = &after[end + 1..];
    }

    expanded.push_str(rest);
    Ok(expanded)
}

fn resolve(
    placeholder: &str,
    values: &TemplateValues<'_>,
) -> Result<Option<String>, TemplateError> {
    let (kind, name) = match placeholder.split_once(':') {
        Some((kind, name)) => (kind, Some(name)),
        None => (placeholder, None),
    };

    if kind == "release_dir" {
        return Ok(values
            .release_dir
            .map(|directory| directory.to_string_lossy().into_owned()));
    }

    if !matches!(kind, "port" | "address" | "listen_address") {
        return Ok(None);
    }

    let name = name.unwrap_or(DEFAULT_PORT_NAME);
    let port = values
        .ports
        .get(name)
        .ok_or_else(|| TemplateError::UnknownPort(name.to_string()))?;

    let value = match kind {
        "port" => port.to_string(),
        "address" => LOCAL_ADDRESS.to_string(),
        "listen_address" => format!("{LOCAL_ADDRESS}:{port}"),
        _ => return Ok(None),
    };
    Ok(Some(value))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("template requires {{release_dir}} but no release is installed")]
    MissingRelease,
    #[error("template references unknown application port '{0}'")]
    UnknownPort(String),
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

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
}
