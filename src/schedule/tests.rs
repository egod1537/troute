use super::*;
use crate::api::OptimizeRouteRequest;

fn problem() -> OptimizationProblem {
    let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-schedule-test",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":10},
                    {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":20},
                    {"id":"D","place_id":"d","open_time":"00:00","close_time":"23:50","stay_minutes":0}
                ],
                "start_time":"09:00"
            }"#,
        )
        .unwrap();
    request.try_into().unwrap()
}

fn matrix() -> TravelTimeMatrix {
    TravelTimeMatrix::new(vec![vec![0; 4]; 4]).unwrap()
}

#[test]
fn reordered_intermediates_keep_both_endpoints_fixed() {
    let problem = problem();
    let solution = SolverSolution {
        visit_order: vec![0, 2, 1, 3],
    };
    let plan = calculate_schedule(&problem, &matrix(), &solution).unwrap();

    assert_eq!(
        plan.stops
            .iter()
            .map(|stop| stop.location_index)
            .collect::<Vec<_>>(),
        vec![0, 2, 1, 3]
    );
    assert_eq!(plan.stops[0].departure_time.unwrap().to_string(), "09:00");
    assert_eq!(plan.stops[1].departure_time.unwrap().to_string(), "09:20");
    assert_eq!(plan.stops[2].departure_time.unwrap().to_string(), "09:30");
    assert_eq!(plan.stops[3].departure_time.unwrap().to_string(), "09:30");
}

#[test]
fn unreachable_arc_is_a_typed_schedule_error_not_a_panic() {
    let problem = problem();
    let matrix = TravelTimeMatrix::new_sparse(vec![
        vec![Some(0), None, Some(0), Some(0)],
        vec![Some(0), Some(0), Some(0), Some(0)],
        vec![Some(0), Some(0), Some(0), Some(0)],
        vec![Some(0), Some(0), Some(0), Some(0)],
    ])
    .unwrap();
    let solution = SolverSolution {
        visit_order: vec![0, 1, 2, 3],
    };

    assert!(matches!(
        calculate_schedule(&problem, &matrix, &solution),
        Err(ScheduleError::UnreachableArc {
            ref from_location_id,
            ref to_location_id,
        }) if from_location_id == "A" && to_location_id == "B"
    ));
}

#[test]
fn moved_start_or_end_is_rejected() {
    let problem = problem();
    for visit_order in [vec![1, 0, 2, 3], vec![0, 1, 3, 2]] {
        let error =
            calculate_schedule(&problem, &matrix(), &SolverSolution { visit_order }).unwrap_err();
        assert!(matches!(error, ScheduleError::InvalidVisitOrder));
    }
}

#[test]
fn schedule_exposes_service_wait_and_stay_times() {
    let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-schedule-fields",
                "start_policy":"FIXED",
                "start_time":"09:00",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"10:00","close_time":"10:30","stay_minutes":20}
                ]
            }"#,
        )
        .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0, 15], vec![15, 0]]).unwrap();
    let plan = calculate_schedule(
        &problem,
        &matrix,
        &SolverSolution {
            visit_order: vec![0, 1],
        },
    )
    .unwrap();

    assert_eq!(plan.stops[1].arrival_time.to_string(), "09:15");
    assert_eq!(plan.stops[1].service_start_time.to_string(), "10:00");
    assert_eq!(plan.stops[1].departure_time.unwrap().to_string(), "10:20");
    assert_eq!(plan.stops[1].wait_minutes, 45);
    assert_eq!(plan.stops[1].stay_minutes, 20);
}

#[test]
fn start_stay_delays_the_first_travel_leg() {
    let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-start-stay",
                "start_policy":"FIXED",
                "start_time":"09:00",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"08:00","close_time":"18:00","stay_minutes":60},
                    {"id":"B","place_id":"b","open_time":"08:00","close_time":"18:00","stay_minutes":0}
                ]
            }"#,
        )
        .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0, 20], vec![20, 0]]).unwrap();
    let plan = calculate_schedule(
        &problem,
        &matrix,
        &SolverSolution {
            visit_order: vec![0, 1],
        },
    )
    .unwrap();

    assert_eq!(plan.stops[0].arrival_time.to_string(), "09:00");
    assert_eq!(plan.stops[0].service_start_time.to_string(), "09:00");
    assert_eq!(plan.stops[0].departure_time.unwrap().to_string(), "10:00");
    assert_eq!(plan.stops[0].stay_minutes, 60);
    assert_eq!(plan.stops[1].arrival_time.to_string(), "10:20");
    assert_eq!(plan.total_travel_minutes, 20);
}

#[test]
fn start_opening_time_adds_wait_before_stay() {
    let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-start-wait",
                "start_policy":"FIXED",
                "start_time":"08:30",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"09:00","close_time":"18:00","stay_minutes":30},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0}
                ]
            }"#,
        )
        .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0; 2]; 2]).unwrap();
    let plan = calculate_schedule(
        &problem,
        &matrix,
        &SolverSolution {
            visit_order: vec![0, 1],
        },
    )
    .unwrap();

    assert_eq!(plan.stops[0].arrival_time.to_string(), "08:30");
    assert_eq!(plan.stops[0].service_start_time.to_string(), "09:00");
    assert_eq!(plan.stops[0].wait_minutes, 30);
    assert_eq!(plan.stops[0].departure_time.unwrap().to_string(), "09:30");
}

#[test]
fn start_service_must_finish_before_close() {
    let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-start-close",
                "start_policy":"FIXED",
                "start_time":"09:00",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"09:00","close_time":"09:30","stay_minutes":60},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0}
                ]
            }"#,
        )
        .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0; 2]; 2]).unwrap();
    let error = calculate_schedule(
        &problem,
        &matrix,
        &SolverSolution {
            visit_order: vec![0, 1],
        },
    )
    .unwrap_err();

    assert!(matches!(
        error,
        ScheduleError::TimeWindowViolation { ref location_id, .. } if location_id == "A"
    ));
}

#[test]
fn service_finishing_after_close_is_infeasible() {
    let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-schedule-close",
                "start_policy":"FIXED",
                "start_time":"09:00",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"09:00","close_time":"09:20","stay_minutes":10}
                ]
            }"#,
        )
        .unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0, 15], vec![15, 0]]).unwrap();
    let error = calculate_schedule(
        &problem,
        &matrix,
        &SolverSolution {
            visit_order: vec![0, 1],
        },
    )
    .unwrap_err();
    assert!(matches!(error, ScheduleError::TimeWindowViolation { .. }));
}
