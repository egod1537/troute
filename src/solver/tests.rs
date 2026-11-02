use super::*;
use crate::{
    api::OptimizeRouteRequest, cancellation::CancellationToken, domain::TimeOfDay,
    matrix::TravelTimeMatrix, schedule::calculate_schedule_from,
};

#[test]
fn shared_evaluator_applies_start_wait_and_stay() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "evaluate-start-service",
        "start_policy": "FIXED",
        "start_time": "08:30",
        "locations": [
            {"id":"A","place_id":"a","open_time":"09:00","close_time":"18:00","stay_minutes":60},
            {"id":"B","place_id":"b","open_time":"00:00","close_time":"18:00","stay_minutes":0}
        ]
    }))
    .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0, 20], vec![20, 0]]).unwrap();
    let metrics = evaluate_solution(
        &DefaultObjectivePolicy,
        SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        },
        &SolverSolution {
            visit_order: vec![0, 1],
        },
    )
    .unwrap();

    assert_eq!(metrics.start_time_slot, 8 * 60 + 30);
    assert_eq!(metrics.finish_time_slot, 10 * 60 + 20);
    assert_eq!(metrics.wait_minutes, 30);
    assert_eq!(metrics.travel_minutes, 20);
    assert_eq!(metrics.score, 50);
}

#[test]
fn fixed_0905_plus_fifteen_minutes_meets_0920_close_exactly() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "minute-boundary-fixed",
        "start_policy": "FIXED",
        "start_time": "09:05",
        "locations": [
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"B","place_id":"b","open_time":"00:00","close_time":"09:20","stay_minutes":0}
        ]
    }))
    .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0, 15], vec![15, 0]]).unwrap();
    let cancellation = CancellationToken::new();
    let result = ExactBitDpSolver::default()
        .solve_detailed(SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &cancellation,
        })
        .unwrap();

    assert_eq!(result.metrics.start_time_slot, 9 * 60 + 5);
    assert_eq!(result.metrics.finish_time_slot, 9 * 60 + 20);
    let plan = calculate_schedule_from(
        &problem,
        &matrix,
        &result.solution,
        TimeOfDay::from_minutes(9 * 60 + 5).unwrap(),
    )
    .unwrap();
    assert_eq!(
        plan.stops.last().unwrap().departure_time.unwrap().minutes(),
        result.metrics.finish_time_slot
    );
}

#[test]
fn short_consecutive_legs_and_waiting_keep_every_minute() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "minute-short-legs",
        "start_policy": "FIXED",
        "start_time": "09:05",
        "locations": [
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"B","place_id":"b","open_time":"09:10","close_time":"10:00","stay_minutes":0},
            {"id":"C","place_id":"c","open_time":"00:00","close_time":"10:00","stay_minutes":0},
            {"id":"D","place_id":"d","open_time":"00:00","close_time":"10:00","stay_minutes":0},
            {"id":"E","place_id":"e","open_time":"00:00","close_time":"10:00","stay_minutes":0}
        ]
    }))
    .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let mut rows = vec![vec![100; 5]; 5];
    for (index, row) in rows.iter_mut().enumerate() {
        row[index] = 0;
    }
    rows[0][1] = 3;
    rows[1][2] = 7;
    rows[2][3] = 9;
    rows[3][4] = 1;
    let matrix = TravelTimeMatrix::new(rows).unwrap();
    let cancellation = CancellationToken::new();
    let result = ExactBitDpSolver::default()
        .solve_detailed(SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &cancellation,
        })
        .unwrap();

    assert_eq!(result.solution.visit_order, vec![0, 1, 2, 3, 4]);
    assert_eq!(result.metrics.finish_time_slot, 9 * 60 + 27);
    assert_eq!(result.metrics.travel_minutes, 20);
    assert_eq!(result.metrics.wait_minutes, 2);
}

#[test]
fn latest_start_is_selected_on_the_minute_boundary() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "minute-boundary-latest",
        "start_policy": "LATEST",
        "start_time": "00:00",
        "locations": [
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"B","place_id":"b","open_time":"00:00","close_time":"09:20","stay_minutes":0}
        ]
    }))
    .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0, 15], vec![15, 0]]).unwrap();
    let result = ExactBitDpSolver::default()
        .solve_detailed(SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        })
        .unwrap();

    assert_eq!(result.metrics.start_time_slot, 9 * 60 + 5);
    assert_eq!(result.metrics.finish_time_slot, 9 * 60 + 20);
}

#[test]
fn exact_solver_uses_reachable_sparse_path_and_ignores_irrelevant_reverse_arc() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "sparse-exact",
        "start_policy": "FIXED",
        "start_time": "09:00",
        "locations": [
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ]
    }))
    .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new_sparse(vec![
        vec![Some(0), Some(0), None],
        vec![None, Some(0), Some(7)],
        vec![None, None, Some(0)],
    ])
    .unwrap();
    let input = SolverInput {
        matrix: &matrix,
        problem: &problem,
        cancellation: &CancellationToken::new(),
    };

    let result = ExactBitDpSolver::default().solve_detailed(input).unwrap();
    assert_eq!(result.solution.visit_order, vec![0, 1, 2]);
    assert_eq!(result.metrics.travel_minutes, 7);

    let blocked = TravelTimeMatrix::new_sparse(vec![
        vec![Some(0), None, None],
        vec![None, Some(0), Some(7)],
        vec![None, None, Some(0)],
    ])
    .unwrap();
    assert!(matches!(
        ExactBitDpSolver::default().solve_detailed(SolverInput {
            matrix: &blocked,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        }),
        Err(SolverError::NoFeasibleRoute)
    ));
}

#[test]
fn best_effort_exact_maximizes_optional_count_before_priority() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "partial-exact-coverage",
            "visit_policy": "BEST_EFFORT_ON_FAILURE",
            "start_policy": "FIXED",
            "start_time": "09:00",
            "locations": [
                {"id":"start","place_id":"s","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"high","priority":100,"place_id":"h","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"low-a","priority":1,"place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"low-b","priority":5,"place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"end","place_id":"e","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ]
        }))
        .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new_sparse(vec![
        vec![Some(0), Some(1), Some(1), None, Some(20)],
        vec![None, Some(0), None, None, Some(1)],
        vec![None, None, Some(0), Some(1), Some(10)],
        vec![None, None, None, Some(0), Some(1)],
        vec![None, None, None, None, Some(0)],
    ])
    .unwrap();
    let result = ExactBitDpSolver::default()
        .solve_detailed(SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        })
        .unwrap();

    assert_eq!(result.solution.visit_order, vec![0, 2, 3, 4]);
    assert_eq!(result.metrics.visited_optional_count, 2);
    assert_eq!(result.metrics.visited_optional_priority, 6);
}

#[test]
fn best_effort_exact_never_skips_an_unreachable_required_location() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "partial-required-blocked",
            "visit_policy": "BEST_EFFORT_ON_FAILURE",
            "start_policy": "FIXED",
            "start_time": "09:00",
            "locations": [
                {"id":"start","place_id":"s","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"required","required":true,"place_id":"r","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"optional","place_id":"o","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"end","place_id":"e","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ]
        }))
        .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new_sparse(vec![
        vec![Some(0), None, Some(1), Some(2)],
        vec![None, Some(0), None, Some(1)],
        vec![None, None, Some(0), Some(1)],
        vec![None, None, None, Some(0)],
    ])
    .unwrap();

    assert!(matches!(
        ExactBitDpSolver::default().solve_detailed(SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        }),
        Err(SolverError::NoFeasibleRoute)
    ));
}
