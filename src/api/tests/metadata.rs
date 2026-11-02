#[test]
fn visit_policy_defaults_and_optional_metadata_are_normalized() {
    let legacy: OptimizationProblem = request_with_locations(&["A", "B", "C"]).try_into().unwrap();
    assert_eq!(legacy.visit_policy(), VisitPolicy::AllRequired);
    assert!(legacy.locations().iter().all(Location::required));

    let partial: OptimizeRouteRequest = serde_json::from_value(json!({
            "job_id": "partial-domain-contract",
            "visit_policy": "BEST_EFFORT_ON_FAILURE",
            "locations": [
                {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0,"required":true},
                {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0,"priority":7},
                {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":0,"required":true}
            ]
        }))
        .unwrap();
    let partial: OptimizationProblem = partial.try_into().unwrap();
    assert_eq!(partial.visit_policy(), VisitPolicy::BestEffortOnFailure);
    assert!(partial.locations()[0].required());
    assert!(!partial.locations()[1].required());
    assert_eq!(partial.locations()[1].priority(), 7);
    assert!(partial.locations()[2].required());

    let invalid: OptimizeRouteRequest = serde_json::from_value(json!({
            "job_id": "partial-invalid-endpoint",
            "visit_policy": "BEST_EFFORT_ON_FAILURE",
            "locations": [
                {"id":"A","required":false,"place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ]
        }))
        .unwrap();
    assert!(matches!(
        OptimizationProblem::try_from(invalid),
        Err(RequestValidationError::RequiredLocationCannotBeOptional { .. })
    ));
}

#[test]
fn dated_departure_uses_iana_zone_and_rejects_dst_gaps() {
    let mut seoul = request_with_locations(&["A", "B"]);
    seoul.country_code = Some("KR".to_owned());
    seoul.travel_date = Some("2026-11-01".to_owned());
    seoul.time_zone = Some("Asia/Seoul".to_owned());
    seoul.start_time = Some("09:00".parse().unwrap());
    let (instant, zone) =
        requested_routing_departure(&seoul, seoul.start_time.expect("test start time"))
            .unwrap()
            .expect("dated request has an instant");
    assert_eq!(instant.to_rfc3339(), "2026-11-01T00:00:00+00:00");
    assert_eq!(zone, "Asia/Seoul");

    let mut gap = seoul.clone();
    gap.country_code = None;
    gap.travel_date = Some("2026-03-08".to_owned());
    gap.time_zone = Some("America/New_York".to_owned());
    gap.start_time = Some("02:30".parse().unwrap());
    assert!(matches!(
        OptimizationProblem::try_from(gap),
        Err(RequestValidationError::NonexistentLocalDeparture { .. })
    ));
}

#[test]
fn dated_departure_requires_both_fields_and_matching_known_country_zone() {
    let mut missing_zone = request_with_locations(&["A", "B"]);
    missing_zone.travel_date = Some("2026-11-01".to_owned());
    assert!(matches!(
        OptimizationProblem::try_from(missing_zone),
        Err(RequestValidationError::TravelDateRequiresTimeZone)
    ));

    let mut mismatch = request_with_locations(&["A", "B"]);
    mismatch.country_code = Some("JP".to_owned());
    mismatch.travel_date = Some("2026-11-01".to_owned());
    mismatch.time_zone = Some("Asia/Seoul".to_owned());
    assert!(matches!(
        OptimizationProblem::try_from(mismatch),
        Err(RequestValidationError::CountryTimeZoneMismatch { .. })
    ));
}

#[test]
fn response_exposes_ranked_solver_candidates_and_best_marker() {
    let problem = OptimizationProblem::try_from(request_with_locations(&["A", "B"])).unwrap();
    let diagnostics = SolverDiagnostics {
        selected_strategy: "exact_bit_dp".to_owned(),
        candidates: vec![SolverCandidate {
            strategy: "exact_bit_dp".to_owned(),
            route: Some(SolverSolution {
                visit_order: vec![0, 1],
            }),
            feasible: true,
            objective_score: Some(SolutionMetrics {
                start_policy: StartPolicy::Latest,
                visited_optional_count: 0,
                visited_optional_priority: 0,
                start_time_slot: 600,
                finish_time_slot: 610,
                travel_minutes: 10,
                wait_minutes: 0,
                score: 10,
            }),
            elapsed: Duration::from_millis(7),
            metadata: SolverCandidateMetadata {
                state_count: Some(2),
                frontier_state_count: Some(2),
                frontier_cell_count: Some(2),
                ..SolverCandidateMetadata::default()
            },
        }],
        total_budget_ms: 30_000,
        total_elapsed_ms: 7,
        baseline_elapsed_ms: 7,
        sa_elapsed_ms: 0,
        sa_run_count: 0,
        global_best_updates: 1,
        termination_reason: "exact_optimum".to_owned(),
        portfolio_telemetry: None,
    };
    let response = OptimizeRouteResponse::from_plan(
        RoutePlan {
            stops: vec![
                ScheduledStop {
                    location_index: 0,
                    arrival_time: TimeOfDay::from_minutes(600).unwrap(),
                    service_start_time: TimeOfDay::from_minutes(600).unwrap(),
                    departure_time: Some(TimeOfDay::from_minutes(600).unwrap()),
                    wait_minutes: 0,
                    stay_minutes: 0,
                },
                ScheduledStop {
                    location_index: 1,
                    arrival_time: TimeOfDay::from_minutes(610).unwrap(),
                    service_start_time: TimeOfDay::from_minutes(610).unwrap(),
                    departure_time: None,
                    wait_minutes: 0,
                    stay_minutes: 0,
                },
            ],
            total_travel_minutes: 10,
        },
        &problem,
        Some(&diagnostics),
    );

    let candidate = &response.solver_candidates.unwrap()[0];
    assert!(candidate.best);
    assert!(candidate.feasible);
    assert_eq!(candidate.route, ["A", "B"]);
    assert_eq!(candidate.elapsed_ms, 7);
    assert_eq!(
        candidate.objective_score.unwrap().latest_start.to_string(),
        "10:00"
    );
    assert_eq!(candidate.metadata.state_count, Some(2));
    assert_eq!(candidate.metadata.frontier_state_count, Some(2));
    assert_eq!(candidate.metadata.frontier_cell_count, Some(2));
}
