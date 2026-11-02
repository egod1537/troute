#[test]
fn routing_remediation_only_returns_modes_verified_by_a_real_query() {
    let modes = Arc::new(Mutex::new(Vec::new()));
    let service = RouteOptimizationService::new(
        AlternativeModeRoutingProvider {
            modes: Arc::clone(&modes),
        },
        UnusedSolver,
    );
    let mut request = valid_request();
    request.travel_mode = Some(TravelMode::Transit);
    let error = service.optimize(request.clone()).unwrap_err();

    let suggestions = service.failure_suggestions(&request, &error, &CancellationToken::new());
    let alternatives = suggestions
        .iter()
        .filter(|suggestion| {
            suggestion.suggestion_type == crate::failure::FailureSuggestionType::ChangeTravelMode
        })
        .collect::<Vec<_>>();

    assert_eq!(alternatives.len(), 1);
    assert_eq!(
        alternatives[0].current_value,
        Some(serde_json::json!("TRANSIT"))
    );
    assert_eq!(
        alternatives[0].suggested_value,
        Some(serde_json::json!("WALKING"))
    );
    assert_eq!(
        *modes.lock().unwrap(),
        [
            TravelMode::Transit,
            TravelMode::Driving,
            TravelMode::Walking,
            TravelMode::Bicycling
        ]
    );
}

#[test]
fn removal_and_split_suggestions_require_a_successful_bounded_probe() {
    let service = RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver)
        .with_remediation_config(RemediationConfig {
            budget: Duration::from_millis(500),
            max_remove_probes: 1,
            max_suggestions: 3,
        });
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "remove-probe",
            "start_policy": "FIXED",
            "start_time": "09:00",
            "locations": [
                {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"blocked","place_id":"b","open_time":"10:00","close_time":"10:10","stay_minutes":20},
                {"id":"keep","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":10},
                {"id":"D","place_id":"d","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ],
            "travel_time_matrix": [
                [0,10,10,10], [10,0,10,10], [10,10,0,10], [10,10,10,0]
            ]
        }))
        .unwrap();

    let suggestions = service.failure_suggestions(
        &request,
        &OptimizationServiceError::Solver(SolverError::NoFeasibleRoute),
        &CancellationToken::new(),
    );

    assert!(suggestions.iter().any(|suggestion| {
        suggestion.suggestion_type == crate::failure::FailureSuggestionType::RemoveLocation
            && suggestion.location_id.as_deref() == Some("blocked")
    }));
    assert!(suggestions.iter().any(|suggestion| {
        suggestion.suggestion_type == crate::failure::FailureSuggestionType::SplitDay
            && suggestion.location_id.as_deref() == Some("blocked")
    }));
    assert!(!suggestions.iter().any(|suggestion| {
        suggestion.suggestion_type == crate::failure::FailureSuggestionType::RemoveLocation
            && suggestion.location_id.as_deref() == Some("keep")
    }));
}
