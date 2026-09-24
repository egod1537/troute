use super::super::{SolutionMetrics, SolverSolution};

/// Exact result details are available without expanding the stable
/// `SolverSolution` hand-off used by the scheduler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactSolveResult {
    pub solution: SolverSolution,
    pub metrics: SolutionMetrics,
    pub stats: ExactSolverStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExactSolverStats {
    pub generated_states: usize,
    pub frontier_states: usize,
    pub frontier_cells: usize,
}
