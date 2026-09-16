use thiserror::Error;

use crate::{domain::OptimizationProblem, matrix::TravelTimeMatrix};

pub struct SolverInput<'a> {
    pub matrix: &'a TravelTimeMatrix,
    pub problem: &'a OptimizationProblem,
}

/// The visit order uses every location index exactly once. It starts at index
/// zero, ends at the final index, and may reorder only intermediate indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverSolution {
    pub visit_order: Vec<usize>,
}

/// Finds an order that minimizes travel time while respecting v0 constraints.
pub trait RouteSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError>;
}

#[derive(Debug, Error)]
pub enum SolverError {
    #[error("no feasible route was found")]
    NoFeasibleRoute,
    #[error("solver failed: {0}")]
    Failed(String),
}
