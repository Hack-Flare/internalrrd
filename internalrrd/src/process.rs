use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::time::{Duration, Instant};

use tokio::process::{Child, Command};

use crate::config::ApplicationConfig;
use crate::supervisor::InstanceId;

/// Everything needed to spawn one application instance.
#[derive(Debug, Clone)]
pub struct ProcessSpec {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub environment: BTreeMap<String, String>,
    pub shutdown_timeout: Duration,
}

impl From<&ApplicationConfig> for ProcessSpec {
    fn from(config: &ApplicationConfig) -> Self {
        Self {
            command: config.command.clone(),
            args: config.args.clone(),
            working_directory: config.working_directory.clone(),
            environment: config.environment.clone(),
            shutdown_timeout: config.shutdown_timeout(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    #[error("failed to start application process: {0}")]
    Spawn(#[source] io::Error),
    #[error("application process did not report a process identifier")]
    MissingPid,
    #[error("failed to inspect application process: {0}")]
    Wait(#[source] io::Error),
    #[error("failed to signal application process: {0}")]
    Signal(#[source] io::Error),
    #[error("failed to kill application process: {0}")]
    Kill(#[source] io::Error),
}

/// A running application instance owned by the supervisor.
#[derive(Debug)]
pub struct ManagedProcess {
    id: InstanceId,
    pid: u32,
    child: Child,
    started_at: Instant,
}

impl ManagedProcess {
    pub(crate) fn spawn(id: InstanceId, spec: &ProcessSpec) -> Result<Self, ProcessError> {
        let mut command = Command::new(&spec.command);
        command.args(&spec.args);
        command.stdin(Stdio::null());
        command.kill_on_drop(true);

        if let Some(working_directory) = &spec.working_directory {
            command.current_dir(working_directory);
        }

        command.envs(&spec.environment);

        let child = command.spawn().map_err(ProcessError::Spawn)?;
        let pid = child.id().ok_or(ProcessError::MissingPid)?;

        Ok(Self {
            id,
            pid,
            child,
            started_at: Instant::now(),
        })
    }

    pub fn id(&self) -> InstanceId {
        self.id
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }

    /// Check whether the instance has exited without blocking.
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, ProcessError> {
        self.child.try_wait().map_err(ProcessError::Wait)
    }

    /// Ask the instance to stop, killing it if it exceeds the timeout.
    pub async fn terminate(&mut self, timeout: Duration) -> Result<(), ProcessError> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }

        self.request_termination()?;

        match tokio::time::timeout(timeout, self.child.wait()).await {
            Ok(result) => {
                result.map_err(ProcessError::Wait)?;
                Ok(())
            }
            Err(_) => {
                tracing::warn!(
                    pid = self.pid,
                    "application process did not stop in time, killing it"
                );
                self.child.start_kill().map_err(ProcessError::Kill)?;
                self.child.wait().await.map_err(ProcessError::Wait)?;
                Ok(())
            }
        }
    }

    #[cfg(unix)]
    fn request_termination(&self) -> Result<(), ProcessError> {
        // SAFETY: the process identifier comes from a child process that is still tracked.
        let result = unsafe { libc::kill(self.pid as libc::pid_t, libc::SIGTERM) };
        if result == 0 {
            return Ok(());
        }

        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(());
        }
        Err(ProcessError::Signal(error))
    }

    #[cfg(not(unix))]
    fn request_termination(&mut self) -> Result<(), ProcessError> {
        self.child.start_kill().map_err(ProcessError::Kill)
    }
}
