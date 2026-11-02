use std::{fmt, time::Duration};

use serde::{Deserialize, Serialize};

use crate::solver::{
    ResourceRequest, SolverCandidate, SolverId, SolverOutcomeKind, SolverSolution,
};

/// Stable identity for one independently scheduled solver invocation.
///
/// A solver may be restarted many times; every restart receives a new task ID
/// while retaining the same [`SolverId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TaskId(u64);

impl TaskId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Immutable definition of one solver invocation.
///
/// Runtime deadlines, cancellation tokens, and acquired leases deliberately
/// live in [`crate::solver::ExecutionContext`] instead of this definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverTask {
    pub task_id: TaskId,
    pub solver_id: SolverId,
    pub timeout: Duration,
    pub seed: Option<u64>,
    pub warm_start: Option<SolverSolution>,
    pub resources: ResourceRequest,
}

impl SolverTask {
    pub fn new(
        task_id: TaskId,
        solver_id: SolverId,
        timeout: Duration,
        resources: ResourceRequest,
    ) -> Self {
        Self {
            task_id,
            solver_id,
            timeout,
            seed: None,
            warm_start: None,
            resources,
        }
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    pub fn with_warm_start(mut self, warm_start: SolverSolution) -> Self {
        self.warm_start = Some(warm_start);
        self
    }
}

/// Lifecycle state kept separately from the immutable task definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskState {
    Queued,
    Running,
    Completed,
    TimedOut,
    Cancelled,
    Failed,
}

/// Validated result of one task execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskResult {
    pub task_id: TaskId,
    pub solver_id: SolverId,
    pub state: TaskState,
    pub candidate: SolverCandidate,
}

impl TaskResult {
    pub fn from_candidate(task: &SolverTask, candidate: SolverCandidate) -> Self {
        let state = match candidate.metadata.outcome {
            Some(SolverOutcomeKind::Completed | SolverOutcomeKind::Infeasible) => {
                TaskState::Completed
            }
            Some(SolverOutcomeKind::Timeout) => TaskState::TimedOut,
            Some(SolverOutcomeKind::Cancelled) => TaskState::Cancelled,
            Some(SolverOutcomeKind::Unsupported | SolverOutcomeKind::Failed) | None => {
                TaskState::Failed
            }
        };
        Self {
            task_id: task.task_id,
            solver_id: task.solver_id.clone(),
            state,
            candidate,
        }
    }
}
