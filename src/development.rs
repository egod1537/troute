//! Deterministic development implementations for exercising the HTTP pipeline.
//!
//! These implementations do not provide real travel times or route
//! optimization. They are deliberately isolated from the HTTP and service
//! layers so production implementations can replace them without changing the
//! wire contract.

use crate::{
    domain::Location,
    matrix::TravelTimeMatrix,
    routing::{RoutingContext, RoutingError, RoutingProvider},
    solver::{RouteSolver, SolverError, SolverInput, SolverSolution},
};

/// Placeholder travel time for every pair of different locations.
pub const DEVELOPMENT_TRAVEL_MINUTES: u32 = 15;

/// Produces a zero diagonal and a fixed travel time for every other pair.
#[derive(Debug, Clone, Copy, Default)]
pub struct DevelopmentRoutingProvider;

impl RoutingProvider for DevelopmentRoutingProvider {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        _context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let rows = (0..locations.len())
            .map(|from| {
                (0..locations.len())
                    .map(|to| {
                        if from == to {
                            0
                        } else {
                            DEVELOPMENT_TRAVEL_MINUTES
                        }
                    })
                    .collect()
            })
            .collect();
        TravelTimeMatrix::new(rows).map_err(|error| RoutingError::Provider(error.to_string()))
    }
}

/// Visits all locations in input order, preserving the fixed endpoints.
///
/// This satisfies the structural solver contract but performs no optimization.
#[derive(Debug, Clone, Copy, Default)]
pub struct DevelopmentRouteSolver;

impl RouteSolver for DevelopmentRouteSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let location_count = input.problem.locations().len();
        if input.matrix.size() != location_count {
            return Err(SolverError::Failed(
                "travel time matrix size does not match the optimization problem".to_owned(),
            ));
        }

        let visit_order = (0..location_count).collect();
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        Ok(SolverSolution { visit_order })
    }
}

#[cfg(test)]
mod tests;
