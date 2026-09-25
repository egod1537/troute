use crate::{
    cancellation::CancellationToken,
    domain::{OptimizationProblem, TimeOfDay},
    matrix::TravelTimeMatrix,
};

use super::{SolverError, SolverRunResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolverProgressEvent {
    StrategyStarted(String),
    StrategyCompleted(String),
    AnnealingRunStarted(u64),
    SelectionStarted(usize),
}

pub trait SolverProgressObserver: Send + Sync {
    fn on_progress(&self, event: SolverProgressEvent);

    fn is_enabled(&self) -> bool {
        true
    }
}

#[derive(Debug, Default)]
pub struct NoopSolverProgressObserver;

impl SolverProgressObserver for NoopSolverProgressObserver {
    fn on_progress(&self, _event: SolverProgressEvent) {}

    fn is_enabled(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy)]
pub struct SolverInput<'a> {
    pub matrix: &'a TravelTimeMatrix,
    pub problem: &'a OptimizationProblem,
    pub cancellation: &'a CancellationToken,
}

/// The visit order uses every location index exactly once. It starts at index
/// zero, ends at the final index, and may reorder only intermediate indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverSolution {
    pub visit_order: Vec<usize>,
}

/// Finds a feasible order according to the implementation's objective policy.
pub trait RouteSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError>;

    /// Runs the solver and optionally returns strategy-level diagnostics.
    /// Implementations that do not orchestrate multiple strategies retain the
    /// original `solve` behavior through this default implementation.
    fn solve_with_diagnostics(
        &self,
        input: SolverInput<'_>,
    ) -> Result<SolverRunResult, SolverError> {
        self.solve(input).map(|solution| SolverRunResult {
            solution,
            diagnostics: None,
        })
    }

    /// Runs the solver with optional high-level progress events. Implementors
    /// can override this without coupling their algorithms to job reporting.
    fn solve_with_diagnostics_and_progress(
        &self,
        input: SolverInput<'_>,
        observer: &dyn SolverProgressObserver,
    ) -> Result<SolverRunResult, SolverError> {
        observer.on_progress(SolverProgressEvent::StrategyStarted(
            "route_solver".to_owned(),
        ));
        let result = self.solve_with_diagnostics(input);
        observer.on_progress(SolverProgressEvent::StrategyCompleted(
            "route_solver".to_owned(),
        ));
        if result.is_ok() {
            observer.on_progress(SolverProgressEvent::SelectionStarted(1));
        }
        result
    }

    /// Selects the schedule departure represented by the solution. Existing
    /// solvers retain the request start; exact solvers may choose a later slot.
    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        _solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        Ok(input.problem.start_time())
    }
}
