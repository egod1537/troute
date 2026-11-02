#[test]
fn debug_duration_is_optional_bounded_and_round_trips() {
    let mut request = request_with_locations(&["A", "B"]);
    request.debug = Some(DebugOptions {
        min_job_duration_ms: Some(MAX_DEBUG_JOB_DURATION_MS),
        shuffle_result_route: None,
        shuffle_seed: None,
    });
    let serialized = serde_json::to_value(&request).unwrap();
    assert_eq!(
        serialized["debug"]["min_job_duration_ms"],
        MAX_DEBUG_JOB_DURATION_MS
    );
    OptimizationProblem::try_from(request).unwrap();

    let mut excessive = request_with_locations(&["A", "B"]);
    excessive.debug = Some(DebugOptions {
        min_job_duration_ms: Some(MAX_DEBUG_JOB_DURATION_MS + 1),
        shuffle_result_route: None,
        shuffle_seed: None,
    });
    assert!(matches!(
        OptimizationProblem::try_from(excessive),
        Err(RequestValidationError::DebugJobDurationTooLong {
            maximum: MAX_DEBUG_JOB_DURATION_MS,
            actual
        }) if actual == MAX_DEBUG_JOB_DURATION_MS + 1
    ));
}

#[test]
fn debug_shuffle_options_are_optional_strict_and_round_trip() {
    let mut request = request_with_locations(&["A", "B", "C"]);
    request.debug = Some(DebugOptions {
        min_job_duration_ms: None,
        shuffle_result_route: Some(true),
        shuffle_seed: Some(1_234),
    });

    let serialized = serde_json::to_value(&request).unwrap();
    assert_eq!(serialized["debug"]["shuffle_result_route"], true);
    assert_eq!(serialized["debug"]["shuffle_seed"], 1_234);
    OptimizationProblem::try_from(request).unwrap();

    let invalid = serde_json::from_value::<OptimizeRouteRequest>(json!({
        "job_id": "route-api-test",
        "locations": [
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ],
        "start_time": "09:00",
        "debug": {"shuffle_result_route": "true"}
    }));
    assert!(invalid.is_err());
}
