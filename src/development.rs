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
mod tests {
    use super::*;
    use crate::{
        api::OptimizeRouteRequest,
        domain::OptimizationProblem,
        solver::{RouteSolver, SolverInput},
    };

    fn problem() -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-development-test",
                "locations": [
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:59","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:59","stay_minutes":0},
                    {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:59","stay_minutes":0}
                ],
                "start_time":"09:00"
            }"#,
        )
        .unwrap();
        request.try_into().unwrap()
    }

    #[test]
    fn routing_provider_builds_the_documented_fixed_matrix() {
        let problem = problem();
        let matrix = DevelopmentRoutingProvider
            .travel_time_matrix(problem.locations(), &RoutingContext::default())
            .unwrap();

        assert_eq!(matrix.travel_minutes(0, 0), Some(0));
        assert_eq!(
            matrix.travel_minutes(0, 1),
            Some(DEVELOPMENT_TRAVEL_MINUTES)
        );
        assert_eq!(
            matrix.travel_minutes(2, 0),
            Some(DEVELOPMENT_TRAVEL_MINUTES)
        );
    }

    #[test]
    fn solver_preserves_the_fixed_start_and_end_locations() {
        let problem = problem();
        let matrix = DevelopmentRoutingProvider
            .travel_time_matrix(problem.locations(), &RoutingContext::default())
            .unwrap();
        let solution = DevelopmentRouteSolver
            .solve(SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &crate::CancellationToken::new(),
            })
            .unwrap();

        assert_eq!(solution.visit_order, vec![0, 1, 2]);
        assert_eq!(problem.start_location().id(), "A");
        assert_eq!(problem.end_location().id(), "C");
        assert_eq!(
            problem
                .intermediate_locations()
                .iter()
                .map(|location| location.id())
                .collect::<Vec<_>>(),
            vec!["B"]
        );
    }
}
