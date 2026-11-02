use std::time::{Duration, Instant};

use thiserror::Error;

use crate::{
    cancellation::CancellationToken,
    solver::{ResourceLease, RunId, SolverId, SolverSolution, SolverTask, TaskId},
};

/// Immutable identity, timing, cancellation, and resource state shared with a
/// single solver run. Dropping the context drops its lease and releases every
/// reserved resource through the common RAII path.
pub struct ExecutionContext {
    job_id: String,
    task_id: TaskId,
    run_id: RunId,
    solver_id: SolverId,
    started_at: Instant,
    global_deadline: Instant,
    local_deadline: Instant,
    cancellation: CancellationToken,
    deterministic_seed: u64,
    resource_lease: ResourceLease,
    warm_start: Option<SolverSolution>,
}

impl ExecutionContext {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        job_id: impl Into<String>,
        run_id: RunId,
        solver_id: SolverId,
        started_at: Instant,
        global_deadline: Instant,
        local_deadline: Instant,
        cancellation: CancellationToken,
        deterministic_seed: u64,
        resource_lease: ResourceLease,
        warm_start: Option<SolverSolution>,
    ) -> Result<Self, ExecutionContextError> {
        let job_id = job_id.into();
        if job_id.trim().is_empty() {
            return Err(ExecutionContextError::EmptyJobId);
        }
        if global_deadline < started_at
            || local_deadline < started_at
            || local_deadline > global_deadline
        {
            return Err(ExecutionContextError::InvalidDeadlineOrder);
        }
        Ok(Self {
            job_id,
            task_id: TaskId::new(run_id.get()),
            run_id,
            solver_id,
            started_at,
            global_deadline,
            local_deadline,
            cancellation,
            deterministic_seed,
            resource_lease,
            warm_start,
        })
    }

    /// Builds the runtime context for a scheduled task after its resource
    /// lease has been acquired.
    #[allow(clippy::too_many_arguments)]
    pub fn for_task(
        job_id: impl Into<String>,
        task: &SolverTask,
        started_at: Instant,
        global_deadline: Instant,
        local_deadline: Instant,
        cancellation: CancellationToken,
        resource_lease: ResourceLease,
    ) -> Result<Self, ExecutionContextError> {
        let job_id = job_id.into();
        if job_id.trim().is_empty() {
            return Err(ExecutionContextError::EmptyJobId);
        }
        if global_deadline < started_at
            || local_deadline < started_at
            || local_deadline > global_deadline
        {
            return Err(ExecutionContextError::InvalidDeadlineOrder);
        }
        Ok(Self {
            job_id,
            task_id: task.task_id,
            run_id: RunId::new(task.task_id.get()),
            solver_id: task.solver_id.clone(),
            started_at,
            global_deadline,
            local_deadline,
            cancellation,
            deterministic_seed: task.seed.unwrap_or_default(),
            resource_lease,
            warm_start: task.warm_start.clone(),
        })
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn task_id(&self) -> TaskId {
        self.task_id
    }

    pub fn solver_id(&self) -> &SolverId {
        &self.solver_id
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn global_deadline(&self) -> Instant {
        self.global_deadline
    }

    pub fn local_deadline(&self) -> Instant {
        self.local_deadline
    }

    pub fn remaining(&self) -> Duration {
        self.local_deadline
            .saturating_duration_since(Instant::now())
    }

    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn deterministic_seed(&self) -> u64 {
        self.deterministic_seed
    }

    pub fn resource_lease(&self) -> &ResourceLease {
        &self.resource_lease
    }

    pub fn warm_start(&self) -> Option<&SolverSolution> {
        self.warm_start.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExecutionContextError {
    #[error("execution context job ID must not be empty")]
    EmptyJobId,
    #[error("execution deadlines must satisfy start <= local <= global")]
    InvalidDeadlineOrder,
}
