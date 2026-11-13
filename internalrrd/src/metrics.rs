use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use protocol::{
    ApplicationMetrics, ComponentMetrics, InstanceMetrics, MetricsReport, ResourceMetrics,
    TrafficMetrics,
};

use crate::supervisor::InstanceInfo;

/// Collect a metrics snapshot.
///
/// This performs blocking file system reads and should run in a blocking task.
pub fn collect(
    application_name: &str,
    deployed_version: Option<String>,
    working_directory: Option<&Path>,
    instances: &[InstanceInfo],
    instance: Option<u64>,
) -> MetricsReport {
    let internalrrd = component_metrics(std::process::id(), working_directory);
    let mut instance_metrics: Vec<InstanceMetrics> = instances
        .iter()
        .map(|instance| InstanceMetrics {
            id: instance.id.get(),
            pid: instance.pid,
            healthy: true,
            uptime_secs: instance.uptime.as_secs(),
            metrics: component_metrics(instance.pid, working_directory),
        })
        .collect();
    let aggregate = aggregate_metrics(working_directory, &instance_metrics);
    if let Some(id) = instance {
        instance_metrics.retain(|metrics| metrics.id == id);
    }

    MetricsReport {
        captured_at_unix: unix_timestamp(),
        internalrrd,
        application: Some(ApplicationMetrics {
            name: application_name.to_string(),
            deployed_version,
            aggregate,
            instances: instance_metrics,
        }),
    }
}

fn component_metrics(pid: u32, storage_path: Option<&Path>) -> ComponentMetrics {
    let (storage_used_bytes, storage_limit_bytes) = match storage_path.and_then(filesystem_usage) {
        Some((used, total)) => (used, Some(total)),
        None => (0, None),
    };

    ComponentMetrics {
        resources: ResourceMetrics {
            cpu_basis_points: process_cpu_basis_points(pid).unwrap_or(0),
            memory_used_bytes: process_memory_bytes(pid).unwrap_or(0),
            memory_limit_bytes: None,
            storage_used_bytes,
            storage_limit_bytes,
        },
        traffic: empty_traffic(),
        custom: Vec::new(),
    }
}

fn aggregate_metrics(
    storage_path: Option<&Path>,
    instances: &[InstanceMetrics],
) -> ComponentMetrics {
    let cpu_basis_points = u32::try_from(
        instances
            .iter()
            .map(|instance| u64::from(instance.metrics.resources.cpu_basis_points))
            .sum::<u64>(),
    )
    .unwrap_or(u32::MAX);
    let memory_used_bytes = instances
        .iter()
        .map(|instance| instance.metrics.resources.memory_used_bytes)
        .sum();
    let (storage_used_bytes, storage_limit_bytes) = match storage_path.and_then(filesystem_usage) {
        Some((used, total)) => (used, Some(total)),
        None => (0, None),
    };

    ComponentMetrics {
        resources: ResourceMetrics {
            cpu_basis_points,
            memory_used_bytes,
            memory_limit_bytes: None,
            storage_used_bytes,
            storage_limit_bytes,
        },
        traffic: empty_traffic(),
        custom: Vec::new(),
    }
}

/// Traffic counters stay at zero until request instrumentation is added.
fn empty_traffic() -> TrafficMetrics {
    TrafficMetrics {
        total_requests: 0,
        requests_last_24h: 0,
        average_requests_per_second_milli: 0,
        average_window_seconds: 0,
    }
}

/// Average CPU usage since the process started.
fn process_cpu_basis_points(pid: u32) -> Option<u32> {
    let stat = read_proc_stat(pid)?;
    let ticks_per_second = clock_ticks_per_second() as f64;
    let uptime = system_uptime_secs()?;
    let started_at = stat.start_time as f64 / ticks_per_second;
    let elapsed = uptime - started_at;
    if elapsed <= 0.0 {
        return Some(0);
    }

    let cpu_seconds = (stat.utime + stat.stime) as f64 / ticks_per_second;
    let basis_points = (cpu_seconds / elapsed) * 10_000.0;
    Some(basis_points.round().clamp(0.0, f64::from(u32::MAX)) as u32)
}

struct ProcStat {
    utime: u64,
    stime: u64,
    start_time: u64,
}

fn read_proc_stat(pid: u32) -> Option<ProcStat> {
    let contents = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, fields) = contents.rsplit_once(')')?;
    let fields: Vec<&str> = fields.split_whitespace().collect();

    Some(ProcStat {
        utime: fields.get(11)?.parse::<u64>().ok()?,
        stime: fields.get(12)?.parse::<u64>().ok()?,
        start_time: fields.get(19)?.parse::<u64>().ok()?,
    })
}

fn system_uptime_secs() -> Option<f64> {
    let contents = fs::read_to_string("/proc/uptime").ok()?;
    contents.split_whitespace().next()?.parse().ok()
}

fn process_memory_bytes(pid: u32) -> Option<u64> {
    let contents = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            let kib = value.split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(kib * 1024);
        }
    }
    None
}

#[cfg(unix)]
fn clock_ticks_per_second() -> u64 {
    // SAFETY: sysconf is called with a valid configuration name.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 { ticks as u64 } else { 100 }
}

#[cfg(not(unix))]
fn clock_ticks_per_second() -> u64 {
    100
}

#[cfg(unix)]
fn filesystem_usage(path: &Path) -> Option<(u64, u64)> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs is called with a valid path and an initialized buffer.
    let mut stats = unsafe { std::mem::zeroed::<libc::statvfs>() };
    // SAFETY: statvfs is called with a valid path and an initialized buffer.
    let result = unsafe { libc::statvfs(path.as_ptr(), &mut stats) };
    if result != 0 {
        return None;
    }

    let block_size = if stats.f_frsize > 0 {
        stats.f_frsize as u64
    } else {
        stats.f_bsize as u64
    };
    let total = stats.f_blocks as u64 * block_size;
    let available = stats.f_bavail as u64 * block_size;
    Some((total.saturating_sub(available), total))
}

#[cfg(not(unix))]
fn filesystem_usage(_path: &Path) -> Option<(u64, u64)> {
    None
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn reads_process_memory() {
        let memory = process_memory_bytes(std::process::id());

        assert!(memory.is_some_and(|bytes| bytes > 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_process_cpu() {
        let cpu = process_cpu_basis_points(std::process::id());

        assert!(cpu.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn reads_filesystem_usage() {
        let usage = filesystem_usage(Path::new("/"));

        assert!(usage.is_some_and(|(used, total)| used <= total && total > 0));
    }
}
