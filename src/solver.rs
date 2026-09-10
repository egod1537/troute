use thiserror::Error;

use crate::{domain::OptimizationProblem, matrix::TravelTimeMatrix};

pub struct SolverInput<'a> {
    pub matrix: &'a TravelTimeMatrix,
    pub problem: &'a OptimizationProblem,
}

/// The visit order uses location indices and must include the start location
/// as both its first and last element. No concrete solving strategy is fixed.
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
