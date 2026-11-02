#[test]
fn zero_and_one_location_are_rejected() {
    for ids in [&[][..], &["A"][..]] {
        let error = OptimizationProblem::try_from(request_with_locations(ids)).unwrap_err();
        assert!(matches!(
            error,
            RequestValidationError::TooFewLocations {
                minimum: 2,
                actual
            } if actual == ids.len()
        ));
    }
}

#[test]
fn two_locations_define_start_and_end() {
    let problem = OptimizationProblem::try_from(request_with_locations(&["A", "B"])).unwrap();

    assert_eq!(problem.start_location().id(), "A");
    assert_eq!(problem.end_location().id(), "B");
    assert!(problem.intermediate_locations().is_empty());
}

#[test]
fn fixed_policy_requires_start_time_and_other_policies_default_to_midnight() {
    let mut fixed = serde_json::to_value(request_with_locations(&["A", "B"])).unwrap();
    fixed["start_policy"] = json!("FIXED");
    fixed.as_object_mut().unwrap().remove("start_time");
    let error = OptimizationProblem::try_from(
        serde_json::from_value::<OptimizeRouteRequest>(fixed).unwrap(),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        RequestValidationError::FixedStartTimeRequired
    ));

    for policy in ["EARLIEST", "LATEST"] {
        let mut value = serde_json::to_value(request_with_locations(&["A", "B"])).unwrap();
        value["start_policy"] = json!(policy);
        value.as_object_mut().unwrap().remove("start_time");
        let problem = OptimizationProblem::try_from(
            serde_json::from_value::<OptimizeRouteRequest>(value).unwrap(),
        )
        .unwrap();
        assert_eq!(problem.start_time().to_string(), "00:00");
    }
}

#[test]
fn location_ids_remain_unique() {
    let error = OptimizationProblem::try_from(request_with_locations(&["A", "A"])).unwrap_err();
    assert!(matches!(
        error,
        RequestValidationError::DuplicateLocationId(id) if id == "A"
    ));
}

#[test]
fn location_times_and_stays_require_ten_minute_units() {
    for minute in [0, 10, 20, 30, 40, 50] {
        let mut request = request_with_locations(&["A", "B"]);
        request.locations[0].open_time = TimeOfDay::from_minutes(9 * 60 + minute).unwrap();
        request.locations[0].close_time = TimeOfDay::from_minutes(10 * 60 + minute).unwrap();
        OptimizationProblem::try_from(request).unwrap();
    }

    for minute in [1, 9, 11, 59] {
        for field in ["open_time", "close_time"] {
            let mut request = request_with_locations(&["A", "B"]);
            let value = TimeOfDay::from_minutes(9 * 60 + minute).unwrap();
            if field == "open_time" {
                request.locations[0].open_time = value;
            } else {
                request.locations[0].close_time = value;
            }
            assert!(matches!(
                OptimizationProblem::try_from(request),
                Err(RequestValidationError::TimeNotTenMinuteAligned {
                    field: actual_field,
                    actual,
                    ..
                }) if actual_field == field && actual == value
            ));
        }
    }

    for stay_minutes in [0, 10, 20] {
        let mut request = request_with_locations(&["A", "B"]);
        request.locations[0].stay_minutes = stay_minutes;
        OptimizationProblem::try_from(request).unwrap();
    }
    for stay_minutes in [1, 15, 25] {
        let mut request = request_with_locations(&["A", "B"]);
        request.locations[0].stay_minutes = stay_minutes;
        assert!(matches!(
            OptimizationProblem::try_from(request),
            Err(RequestValidationError::StayMinutesNotTenMinuteMultiple {
                actual,
                ..
            }) if actual == stay_minutes
        ));
    }
}

#[test]
fn travel_mode_is_optional_strict_and_round_trips() {
    let omitted = request_with_locations(&["A", "B"]);
    assert_eq!(omitted.travel_mode, None);
    assert!(serde_json::to_value(&omitted).unwrap()["travel_mode"].is_null());

    for (wire_value, expected) in [
        ("TRANSIT", TravelMode::Transit),
        ("DRIVING", TravelMode::Driving),
        ("WALKING", TravelMode::Walking),
        ("BICYCLING", TravelMode::Bicycling),
    ] {
        let mut value = serde_json::to_value(request_with_locations(&["A", "B"])).unwrap();
        value["travel_mode"] = json!(wire_value);
        let request: OptimizeRouteRequest = serde_json::from_value(value).unwrap();
        assert_eq!(request.travel_mode, Some(expected));
        assert_eq!(
            serde_json::to_value(request).unwrap()["travel_mode"],
            wire_value
        );
    }

    for invalid in ["transit", "DRIVE", "CAR", "PUBLIC_TRANSIT", "FLYING"] {
        let mut value = serde_json::to_value(request_with_locations(&["A", "B"])).unwrap();
        value["travel_mode"] = json!(invalid);
        assert!(serde_json::from_value::<OptimizeRouteRequest>(value).is_err());
    }
}

#[test]
fn supplied_matrix_is_validated_and_allows_empty_place_ids() {
    let mut request = request_with_locations(&["A", "B"]);
    request.locations[0].place_id.clear();
    request.locations[1].place_id.clear();
    request.travel_time_matrix = Some(vec![vec![0, 7], vec![9, 0]]);
    OptimizationProblem::try_from(request).unwrap();

    let mut wrong_size = request_with_locations(&["A", "B"]);
    wrong_size.travel_time_matrix = Some(vec![vec![0]]);
    assert!(matches!(
        OptimizationProblem::try_from(wrong_size),
        Err(RequestValidationError::TravelTimeMatrixSizeMismatch {
            expected: 2,
            actual: 1
        })
    ));

    let mut nonzero_diagonal = request_with_locations(&["A", "B"]);
    nonzero_diagonal.travel_time_matrix = Some(vec![vec![1, 7], vec![9, 0]]);
    assert!(matches!(
        OptimizationProblem::try_from(nonzero_diagonal),
        Err(RequestValidationError::TravelTimeMatrixDiagonalNonzero {
            index: 0,
            actual: 1
        })
    ));
}

#[test]
fn sparse_matrix_accepts_null_arcs_and_rejects_invalid_diagonal() {
    let request: OptimizeRouteRequest = serde_json::from_value(json!({
        "job_id": "sparse-api-matrix",
        "locations": [
            {"id":"A","place_id":"","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"B","place_id":"","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"C","place_id":"","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ],
        "sparse_travel_time_matrix": [
            [0, 0, null],
            [null, 0, 7],
            [3, null, 0]
        ]
    }))
    .unwrap();
    assert!(OptimizationProblem::try_from(request).is_ok());

    let mut invalid = request_with_locations(&["A", "B"]);
    invalid.sparse_travel_time_matrix = Some(vec![vec![None, Some(1)], vec![Some(1), Some(0)]]);
    assert!(matches!(
        OptimizationProblem::try_from(invalid),
        Err(RequestValidationError::TravelTimeMatrixDiagonalUnreachable { index: 0 })
    ));

    let mut duplicate = request_with_locations(&["A", "B"]);
    duplicate.travel_time_matrix = Some(vec![vec![0, 1], vec![1, 0]]);
    duplicate.sparse_travel_time_matrix = Some(vec![vec![Some(0), None], vec![None, Some(0)]]);
    assert!(matches!(
        OptimizationProblem::try_from(duplicate),
        Err(RequestValidationError::MultipleTravelTimeMatrices)
    ));
}
