use std::collections::BTreeMap;
use std::fmt;
use std::process::ExitStatus;
use std::time::Duration;

use crate::process::{ManagedProcess, ProcessError, ProcessSpec};

/// A stable identifier for one application instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(u64);

impl InstanceId {
    fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A snapshot of a tracked application instance.
#[derive(Debug, Clone)]
pub struct InstanceInfo {
    pub id: InstanceId,
    pub pid: u32,
    pub uptime: Duration,
}

/// An application instance that has exited and been reaped.
#[derive(Debug, Clone)]
pub struct ProcessExit {
    pub id: InstanceId,
    pub status: ExitStatus,
    pub ran_for: Duration,
}

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("unknown instance {0}")]
    UnknownInstance(InstanceId),
    #[error("instance identifier space exhausted")]
    InstanceIdsExhausted,
    #[error(transparent)]
    Process(#[from] ProcessError),
}

/// Owns application instances and their lifecycle.
#[derive(Debug, Default)]
pub struct Supervisor {
    next_id: u64,
    instances: BTreeMap<InstanceId, ManagedProcess>,
}

impl Supervisor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new application instance from a fully resolved process spec.
    pub fn start_with(&mut self, spec: ProcessSpec) -> Result<InstanceId, SupervisorError> {
        let id = InstanceId::new(self.next_id);
        let process = ManagedProcess::spawn(id, &spec)?;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or(SupervisorError::InstanceIdsExhausted)?;
        self.instances.insert(id, process);
        Ok(id)
    }

    /// Stop one instance, waiting up to its configured shutdown timeout.
    pub async fn stop(&mut self, id: InstanceId) -> Result<(), SupervisorError> {
        let mut process = self
            .instances
            .remove(&id)
            .ok_or(SupervisorError::UnknownInstance(id))?;
        process.terminate().await?;
        Ok(())
    }

    /// Stop every tracked instance, attempting all of them even if one fails.
    pub async fn stop_all(&mut self) -> Result<(), SupervisorError> {
        let ids: Vec<InstanceId> = self.instances.keys().copied().collect();
        let mut first_error = None;

        for id in ids {
            if let Err(error) = self.stop(id).await {
                first_error.get_or_insert(error);
            }
        }

        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Reap instances that exited on their own.
    pub fn poll(&mut self) -> Result<Vec<ProcessExit>, SupervisorError> {
        let mut finished = Vec::new();

        for (id, process) in &mut self.instances {
            if let Some(status) = process.try_wait()? {
                finished.push((*id, status));
            }
        }

        let mut exited = Vec::new();
        for (id, status) in finished {
            if let Some(process) = self.instances.remove(&id) {
                exited.push(ProcessExit {
                    id,
                    status,
                    ran_for: process.uptime(),
                });
            }
        }

        Ok(exited)
    }

    /// Snapshot the instances that are currently tracked.
    pub fn running(&self) -> Vec<InstanceInfo> {
        self.instances
            .values()
            .map(|process| InstanceInfo {
                id: process.id(),
                pid: process.pid(),
                uptime: process.uptime(),
            })
            .collect()
    }

    pub fn is_running(&self, id: InstanceId) -> bool {
        self.instances.contains_key(&id)
    }
}
