use std::time::Duration;

use crate::solver::{
    ResourceRequest, SolverCandidateMetadata, SolverError, SolverId, SolverInput,
    SolverProgressEvent, SolverProgressObserver, SolverSolution,
};

use super::SolverOrchestratorConfig;

/// Raw strategy output before the common task executor validates it with the
/// configured objective evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyExecution {
    pub solution: SolverSolution,
    pub metadata: SolverCandidateMetadata,
}

/// Adapter implemented by every built-in and application-specific solver.
pub trait OrchestratedStrategy: Send + Sync {
    fn name(&self) -> &str;

    fn is_applicable(&self, _location_count: usize) -> bool {
        true
    }

    fn is_applicable_to(&self, problem: &crate::domain::OptimizationProblem) -> bool {
        self.is_applicable(problem.locations().len())
    }

    fn preserves_result_on_cancellation(&self) -> bool {
        false
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError>;
}

pub(super) fn report_progress(
    observer: &dyn SolverProgressObserver,
    event: impl FnOnce() -> SolverProgressEvent,
) {
    if observer.is_enabled() {
        observer.on_progress(event());
    }
}

pub(super) fn solver_id_for_strategy(strategy: &str) -> SolverId {
    let stable = if strategy.starts_with("sa_") {
        "sa"
    } else {
        strategy
    };
    SolverId::new(stable).unwrap_or_else(|_| SolverId::new("custom").expect("static ID is valid"))
}

pub(super) fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(super) fn resource_request(
    config: &SolverOrchestratorConfig,
    strategy: &str,
) -> ResourceRequest {
    let solver_id = solver_id_for_strategy(strategy);
    let native_threads = if solver_id.as_str() == "mip_scip" {
        config.mip_scip_threads
    } else {
        1
    };
    ResourceRequest {
        logical_cpu_slots: native_threads,
        native_threads,
        external_processes: usize::from(solver_id.as_str() == "lkh3"),
        memory_hint_bytes: (solver_id.as_str() == "mip_scip")
            .then(|| (config.mip_scip_memory_mb as u64).saturating_mul(1024 * 1024)),
    }
}
