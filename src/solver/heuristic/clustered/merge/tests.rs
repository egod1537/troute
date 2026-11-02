use crate::{
    api::OptimizeRouteRequest,
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    solver::{Cluster, ExactBitDpSolver, SolverInput},
};

use super::solve_cluster;

fn problem(stays: [u32; 4], first_member_close: &str) -> OptimizationProblem {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "completed-cluster-anchor",
            "start_policy": "FIXED",
            "start_time": "09:00",
            "locations": [
                {"id":"start","place_id":"p0","open_time":"00:00","close_time":"23:50","stay_minutes":stays[0]},
                {"id":"previous","place_id":"p1","open_time":"00:00","close_time":"23:50","stay_minutes":stays[1]},
                {"id":"first","place_id":"p2","open_time":"00:00","close_time":first_member_close,"stay_minutes":stays[2]},
                {"id":"end","place_id":"p3","open_time":"00:00","close_time":"23:50","stay_minutes":stays[3]}
            ]
        }))
        .unwrap();
    request.try_into().unwrap()
}

#[test]
fn completed_start_anchor_does_not_repeat_its_sixty_minute_stay() {
    let problem = problem([60, 0, 0, 0], "10:10");
    let matrix = TravelTimeMatrix::new(vec![
        vec![0, 10, 10, 10],
        vec![10, 0, 10, 10],
        vec![10, 10, 0, 10],
        vec![10, 10, 10, 0],
    ])
    .unwrap();
    let cancellation = CancellationToken::new();
    let input = SolverInput {
        matrix: &matrix,
        problem: &problem,
        cancellation: &cancellation,
    };

    let solved = solve_cluster(
        &ExactBitDpSolver::default(),
        &input,
        0,
        60, // 09:00 + the start's 60-minute stay
        &Cluster::new(vec![2]),
        &[3],
    )
    .unwrap();

    assert_eq!(solved.order, vec![2]);
}

#[test]
fn completed_inter_cluster_anchor_does_not_repeat_its_stay() {
    let problem = problem([0, 30, 0, 0], "10:10");
    let matrix = TravelTimeMatrix::new(vec![
        vec![0, 10, 10, 10],
        vec![10, 0, 10, 10],
        vec![10, 10, 0, 10],
        vec![10, 10, 10, 0],
    ])
    .unwrap();
    let cancellation = CancellationToken::new();
    let input = SolverInput {
        matrix: &matrix,
        problem: &problem,
        cancellation: &cancellation,
    };

    let solved = solve_cluster(
        &ExactBitDpSolver::default(),
        &input,
        1,
        60, // location 1 has already completed its 30-minute stay
        &Cluster::new(vec![2]),
        &[3],
    )
    .unwrap();

    assert_eq!(solved.order, vec![2]);
}
