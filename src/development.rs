//! Deterministic development implementations for exercising the HTTP pipeline.
//!
//! These implementations do not provide real travel times or route
//! optimization. They are deliberately isolated from the HTTP and service
//! layers so production implementations can replace them without changing the
//! wire contract.

use crate::{
    domain::Location,
    matrix::TravelTimeMatrix,
    routing::{RoutingError, RoutingProvider},
    solver::{RouteSolver, SolverError, SolverInput, SolverSolution},
};

/// Placeholder travel time for every pair of different locations.
pub const DEVELOPMENT_TRAVEL_MINUTES: u32 = 15;

/// Produces a zero diagonal and a fixed travel time for every other pair.
#[derive(Debug, Clone, Copy, Default)]
pub struct DevelopmentRoutingProvider;

impl RoutingProvider for DevelopmentRoutingProvider {
    fn travel_time_matrix(&self, locations: &[Location]) -> Result<TravelTimeMatrix, RoutingError> {
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

/// Visits non-start locations in input order and then returns to the start.
///
/// This satisfies the structural solver contract but performs no optimization.
#[derive(Debug, Clone, Copy, Default)]
pub struct DevelopmentRouteSolver;

impl RouteSolver for DevelopmentRouteSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        let location_count = input.problem.locations().len();
        if input.matrix.size() != location_count {
            return Err(SolverError::Failed(
                "travel time matrix size does not match the optimization problem".to_owned(),
            ));
        }

        let start = input.problem.start_index();
        let mut visit_order = Vec::with_capacity(location_count + 1);
        visit_order.push(start);
        visit_order.extend((0..location_count).filter(|&index| index != start));
        visit_order.push(start);
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

    fn problem_with_start_at(index: usize) -> OptimizationProblem {
        let mut request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "locations": [
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:59","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:59","stay_minutes":0},
                    {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:59","stay_minutes":0}
                ],
                "start_location_id":"A",
                "start_time":"09:00"
            }"#,
        )
        .unwrap();
        request.start_location_id = request.locations[index].id.clone();
        request.try_into().unwrap()
    }

    #[test]
    fn routing_provider_builds_the_documented_fixed_matrix() {
        let problem = problem_with_start_at(0);
        let matrix = DevelopmentRoutingProvider
            .travel_time_matrix(problem.locations())
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
    fn solver_starts_and_ends_at_the_selected_location() {
        let problem = problem_with_start_at(1);
        let matrix = DevelopmentRoutingProvider
            .travel_time_matrix(problem.locations())
            .unwrap();
        let solution = DevelopmentRouteSolver
            .solve(SolverInput {
                matrix: &matrix,
                problem: &problem,
            })
            .unwrap();

        assert_eq!(solution.visit_order, vec![1, 0, 2, 1]);
    }
}
