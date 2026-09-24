use std::cmp::Ordering;

use crate::domain::StartPolicy;

use super::{SolverError, SolverInput, SolverSolution};

/// Values exposed to replaceable objective policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolutionMetrics {
    pub start_policy: StartPolicy,
    pub start_time_slot: u16,
    pub finish_time_slot: u16,
    pub travel_minutes: u32,
    pub wait_minutes: u32,
    pub score: u64,
}

/// Keeps score construction and terminal ordering out of DP transitions.
pub trait ObjectivePolicy: Send + Sync {
    fn score(&self, travel_minutes: u32, wait_minutes: u32) -> u64;

    /// Scalar used only by the replaceable two-axis Pareto frontier. The
    /// default preserves legacy custom policies; the built-in policy encodes
    /// its travel-then-wait lexicographic order exactly.
    fn frontier_cost(&self, travel_minutes: u32, wait_minutes: u32) -> u64 {
        self.score(travel_minutes, wait_minutes)
    }

    /// `Ordering::Less` means that `left` is preferred.
    fn compare(&self, left: &SolutionMetrics, right: &SolutionMetrics) -> Ordering;
}

/// Evaluates and compares complete routes. The blanket implementation keeps
/// feasible-route scoring identical across exact and heuristic solvers.
pub trait ObjectiveEvaluator: Send + Sync {
    fn evaluate(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<SolutionMetrics, SolverError>;

    /// `Ordering::Less` means that `left` is preferred.
    fn compare(&self, left: &SolutionMetrics, right: &SolutionMetrics) -> Ordering;
}
