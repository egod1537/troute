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
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                    {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":0}
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
