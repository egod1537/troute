use super::*;
use crate::{
    api::OptimizeRouteRequest, cancellation::CancellationToken, domain::OptimizationProblem,
    matrix::TravelTimeMatrix, solver::evaluate_solution,
};

fn solve(request: serde_json::Value, rows: Vec<Vec<Option<u32>>>) -> StrategyExecution {
    let request: OptimizeRouteRequest = serde_json::from_value(request).unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new_sparse(rows).unwrap();
    let cancellation = CancellationToken::new();
    let input = SolverInput {
        problem: &problem,
        matrix: &matrix,
        cancellation: &cancellation,
    };
    let result = ScipMipStrategy::new(ScipMipConfig {
        time_limit: Duration::from_secs(5),
        threads: 1,
        memory_mb: 512,
        seed: 42,
    })
    .unwrap()
    .execute(input)
    .unwrap();
    evaluate_solution(
        &crate::solver::DefaultObjectivePolicy,
        input,
        &result.solution,
    )
    .unwrap();
    result
}

#[test]
fn solves_directed_sparse_full_visit_problem() {
    let result = solve(
        serde_json::json!({
            "job_id": "scip-full",
            "start_policy": "FIXED",
            "start_time": "09:00",
            "locations": [
                {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":10},
                {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ]
        }),
        vec![
            vec![Some(0), Some(5), Some(30)],
            vec![None, Some(0), Some(5)],
            vec![None, None, Some(0)],
        ],
    );
    assert_eq!(result.solution.visit_order, vec![0, 1, 2]);
    assert_eq!(
        result.metadata.proof_status,
        Some(SolverProofStatus::Optimal)
    );
}

#[test]
fn partial_model_maximizes_count_then_priority() {
    let result = solve(
        serde_json::json!({
            "job_id": "scip-partial",
            "visit_policy": "BEST_EFFORT_ON_FAILURE",
            "start_policy": "FIXED",
            "start_time": "09:00",
            "locations": [
                {"id":"A","place_id":"a","required":true,"open_time":"09:00","close_time":"10:00","stay_minutes":0},
                {"id":"high","place_id":"h","required":false,"priority":100,"open_time":"09:00","close_time":"09:20","stay_minutes":10},
                {"id":"low1","place_id":"l1","required":false,"priority":1,"open_time":"09:00","close_time":"10:00","stay_minutes":0},
                {"id":"low2","place_id":"l2","required":false,"priority":1,"open_time":"09:00","close_time":"10:00","stay_minutes":0},
                {"id":"Z","place_id":"z","required":true,"open_time":"09:00","close_time":"10:00","stay_minutes":0}
            ]
        }),
        vec![
            vec![Some(0), Some(15), Some(5), Some(30), Some(50)],
            vec![None, Some(0), None, None, Some(30)],
            vec![None, None, Some(0), Some(5), Some(20)],
            vec![None, None, None, Some(0), Some(5)],
            vec![None, None, None, None, Some(0)],
        ],
    );
    assert_eq!(result.solution.visit_order, vec![0, 2, 3, 4]);
}
