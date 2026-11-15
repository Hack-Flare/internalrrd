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
