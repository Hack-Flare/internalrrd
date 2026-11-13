use std::path::Path;

/// Values substituted into instance command templates.
pub struct TemplateValues<'a> {
    pub port: u16,
    pub release_dir: Option<&'a Path>,
}

/// Expand the supported placeholders in a command template.
///
/// Supported placeholders are `{port}`, `{address}`, `{listen_address}`, and `{release_dir}`.
/// Unknown placeholders are left unchanged.
pub fn expand(template: &str, values: &TemplateValues<'_>) -> Result<String, TemplateError> {
    if template.contains("{release_dir}") && values.release_dir.is_none() {
        return Err(TemplateError::MissingRelease);
    }

    let address = format!("127.0.0.1:{}", values.port);
    let mut expanded = template
        .replace("{port}", &values.port.to_string())
        .replace("{address}", "127.0.0.1")
        .replace("{listen_address}", &address);

    if let Some(release_dir) = values.release_dir {
        expanded = expanded.replace("{release_dir}", release_dir.to_string_lossy().as_ref());
    }

    Ok(expanded)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("template requires {{release_dir}} but no release is installed")]
    MissingRelease,
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn values<'a>(port: u16, release_dir: Option<&'a Path>) -> TemplateValues<'a> {
        TemplateValues { port, release_dir }
    }

    #[test]
    fn expands_port_placeholders() -> Result<(), TemplateError> {
        let expanded = expand(
            "--port {port} --listen {listen_address} --host {address}",
            &values(30001, None),
        )?;

        assert_eq!(
            expanded,
            "--port 30001 --listen 127.0.0.1:30001 --host 127.0.0.1"
        );
        Ok(())
    }

    #[test]
    fn expands_release_directories() -> Result<(), TemplateError> {
        let release = PathBuf::from("/srv/releases/1.2.3");
        let expanded = expand("{release_dir}/bin/example", &values(30001, Some(&release)))?;

        assert_eq!(expanded, "/srv/releases/1.2.3/bin/example");
        Ok(())
    }

    #[test]
    fn rejects_release_placeholders_without_a_release() {
        assert_eq!(
            expand("{release_dir}/bin/example", &values(30001, None)),
            Err(TemplateError::MissingRelease)
        );
    }

    #[test]
    fn preserves_unknown_placeholders() -> Result<(), TemplateError> {
        let expanded = expand("{unknown}", &values(30001, None))?;

        assert_eq!(expanded, "{unknown}");
        Ok(())
    }
}
