use super::*;
use crate::{
    api::OptimizeRouteRequest, cancellation::CancellationToken, domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
};

fn input() -> (OptimizationProblem, TravelTimeMatrix, CancellationToken) {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "lkh-transform",
        "locations": [
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ]
    }))
    .unwrap();
    (
        request.try_into().unwrap(),
        TravelTimeMatrix::new_sparse(vec![
            vec![Some(0), Some(0), Some(9)],
            vec![None, Some(0), Some(1)],
            vec![None, None, Some(0)],
        ])
        .unwrap(),
        CancellationToken::new(),
    )
}

#[test]
fn open_path_transformation_fixes_start_end_and_preserves_zero_arcs() {
    let (problem, matrix, cancellation) = input();
    let transformation = OpenPathTransformation::new(SolverInput {
        problem: &problem,
        matrix: &matrix,
        cancellation: &cancellation,
    })
    .unwrap();
    assert_eq!(transformation.weights[0][1], 1);
    assert_eq!(transformation.weights[2][3], 1);
    assert_eq!(transformation.weights[3][0], 1);
    assert_eq!(
        transformation.decode(vec![0, 1, 2, 3]).unwrap(),
        vec![0, 1, 2]
    );
    assert!(transformation.decode(vec![0, 2, 1, 3]).is_err());
}

#[test]
fn tour_parser_rejects_duplicates_and_out_of_range_nodes() {
    assert_eq!(
        parse_tour("NAME: x\nTOUR_SECTION\n1\n2\n3\n-1\n", 3).unwrap(),
        vec![0, 1, 2]
    );
    assert!(parse_tour("TOUR_SECTION\n1\n1\n3\n-1\n", 3).is_err());
    assert!(parse_tour("TOUR_SECTION\n1\n2\n4\n-1\n", 3).is_err());
}
