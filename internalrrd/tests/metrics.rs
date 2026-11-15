use std::error::Error;

use internalrrd::metrics::collect;

#[test]
fn collects_an_application_report() {
    let report = collect("example", None, None, &[], None);

    assert!(report.captured_at_unix > 0);
    let Some(application) = report.application else {
        panic!("application metrics were not collected");
    };
    assert_eq!(application.name, "example");
    assert!(application.instances.is_empty());
}

#[cfg(target_os = "linux")]
#[test]
fn reports_process_memory() {
    let report = collect("example", None, None, &[], None);

    assert!(report.internalrrd.resources.memory_used_bytes > 0);
}

#[cfg(unix)]
#[test]
fn reports_filesystem_usage() -> Result<(), Box<dyn Error>> {
    let temporary = tempfile::tempdir()?;
    let report = collect("example", None, Some(temporary.path()), &[], None);
    let resources = report.internalrrd.resources;
    let limit = resources
        .storage_limit_bytes
        .ok_or("storage limit was not reported")?;

    assert!(limit > 0);
    assert!(resources.storage_used_bytes <= limit);
    Ok(())
}
