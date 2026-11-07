#[test]
fn cancelled_pipeline_stops_before_matrix_and_emits_no_error_or_result() {
    let service = RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver);
    let reporter = InMemoryReporter::default();
    let cancellation = CancellationToken::new();
    cancellation.cancel();

    let error = service
        .optimize_with_cancellation(valid_request(), &reporter, &cancellation)
        .unwrap_err();

    assert!(matches!(error, OptimizationServiceError::Cancelled));
    assert_eq!(
        reporter.events(),
        vec![RecordedEvent::Progress(ProgressStage::Accepted, 0)]
    );
}

#[test]
fn routing_failure_reports_a_stable_error() {
    let service = RouteOptimizationService::new(FailingRoutingProvider, UnusedSolver);
    let reporter = InMemoryReporter::default();

    assert!(service
        .optimize_with_reporter(valid_request(), &reporter)
        .is_err());

    let events = reporter.events();
    assert_eq!(
        events.last(),
        Some(&RecordedEvent::Error(
            OptimizationErrorCode::RoutingUnavailable
        ))
    );
    assert!(matches!(
        events.get(events.len() - 2),
        Some(RecordedEvent::Progress(
            ProgressStage::FetchingTravelTimes,
            20
        ))
    ));
}

#[test]
fn supplied_matrix_bypasses_the_routing_provider() {
    let service = RouteOptimizationService::new(FailingRoutingProvider, DevelopmentRouteSolver);
    let mut request = valid_request();
    request.travel_mode = Some(TravelMode::Driving);
    request.locations[0].place_id.clear();
    request.locations[1].place_id.clear();
    request.travel_time_matrix = Some(vec![vec![0, 7], vec![9, 0]]);

    let response = service.optimize(request).unwrap();

    assert_eq!(response.total_travel_minutes, 7);
}

#[test]
fn euclidean_matrix_bypasses_the_routing_provider() {
    let service = RouteOptimizationService::new(FailingRoutingProvider, DevelopmentRouteSolver);
    let mut request = valid_request();
    request.travel_mode = Some(TravelMode::Walking);
    request.use_euclidean_matrix = true;
    request.locations[0].place_id.clear();
    request.locations[0].latitude = Some(0.0);
    request.locations[0].longitude = Some(0.0);
    request.locations[1].place_id.clear();
    request.locations[1].latitude = Some(0.0);
    request.locations[1].longitude = Some(0.01);

    let response = service.optimize(request).unwrap();

    assert_eq!(response.total_travel_minutes, 14);
    assert_eq!(response.selected_provider, None);
}

#[test]
fn process_default_enables_euclidean_matrix_without_request_flag() {
    let service = RouteOptimizationService::new(FailingRoutingProvider, DevelopmentRouteSolver)
        .with_default_euclidean_matrix(true);
    let mut request = valid_request();
    request.travel_mode = Some(TravelMode::Walking);
    request.use_euclidean_matrix = false;
    request.locations[0].place_id.clear();
    request.locations[0].latitude = Some(0.0);
    request.locations[0].longitude = Some(0.0);
    request.locations[1].place_id.clear();
    request.locations[1].latitude = Some(0.0);
    request.locations[1].longitude = Some(0.01);

    let response = service.optimize(request).unwrap();

    assert_eq!(response.total_travel_minutes, 14);
    assert_eq!(response.selected_provider, None);
}

#[test]
fn supplied_matrix_takes_precedence_over_process_euclidean_default() {
    let service = RouteOptimizationService::new(FailingRoutingProvider, DevelopmentRouteSolver)
        .with_default_euclidean_matrix(true);
    let mut request = valid_request();
    request.travel_time_matrix = Some(vec![vec![0, 7], vec![9, 0]]);

    let response = service.optimize(request).unwrap();

    assert_eq!(response.total_travel_minutes, 7);
}

#[test]
fn request_travel_mode_reaches_routing_context_and_defaults_to_transit() {
    for (requested, expected) in [
        (None, TravelMode::Transit),
        (Some(TravelMode::Transit), TravelMode::Transit),
        (Some(TravelMode::Driving), TravelMode::Driving),
        (Some(TravelMode::Walking), TravelMode::Walking),
        (Some(TravelMode::Bicycling), TravelMode::Bicycling),
    ] {
        let modes = Arc::new(Mutex::new(Vec::new()));
        let service = RouteOptimizationService::new(
            CapturingRoutingProvider {
                modes: Arc::clone(&modes),
            },
            DevelopmentRouteSolver,
        );
        let mut request = valid_request();
        request.travel_mode = requested;

        service.optimize(request).unwrap();

        assert_eq!(*modes.lock().unwrap(), [expected]);
    }
}

#[test]
fn matrix_preview_and_optimization_share_the_injected_matrix_service() {
    let modes = Arc::new(Mutex::new(Vec::new()));
    let matrix_service = Arc::new(MatrixService::new(CapturingRoutingProvider {
        modes: Arc::clone(&modes),
    }));
    let service = RouteOptimizationService::with_matrix_service(
        Arc::clone(&matrix_service),
        DevelopmentRouteSolver,
    );
    let mut request = valid_request();
    request.travel_mode = Some(TravelMode::Driving);

    service.build_travel_time_matrix(request.clone()).unwrap();
    service.optimize(request).unwrap();

    assert_eq!(Arc::strong_count(&matrix_service), 2);
    assert_eq!(modes.lock().unwrap().as_slice(), [TravelMode::Driving; 2]);
}

#[test]
fn dated_route_revalidates_each_selected_leg_at_its_actual_departure() {
    let leg_departures = Arc::new(Mutex::new(Vec::new()));
    let service = RouteOptimizationService::new(
        TimeDependentRoutingProvider {
            leg_departures: Arc::clone(&leg_departures),
        },
        ExactBitDpSolver::default(),
    );
    let mut request = valid_request();
    request.start_policy = Some(crate::domain::StartPolicy::Fixed);
    request.start_time = Some(TimeOfDay::from_minutes(9 * 60).unwrap());
    request.travel_date = Some("2026-11-02".to_owned());
    request.time_zone = Some("Asia/Seoul".to_owned());
    request.country_code = Some("KR".to_owned());

    let response = service.optimize(request).unwrap();

    assert_eq!(response.total_travel_minutes, 25);
    let revalidation = response.time_dependent_revalidation.unwrap();
    assert!(revalidation.attempted);
    assert!(revalidation.applied);
    assert_eq!(revalidation.revalidated_leg_count, 1);
    assert_eq!(revalidation.changed_leg_count, 1);
    assert_eq!(revalidation.max_delta_minutes, 10);
    assert_eq!(revalidation.schedule_feasible, Some(true));
    assert_eq!(
        leg_departures.lock().unwrap()[0].to_rfc3339(),
        "2026-11-02T00:00:00+00:00"
    );
}

#[test]
fn no_feasible_route_reports_the_contract_error_code() {
    let service = RouteOptimizationService::new(FixedRoutingProvider, NoFeasibleSolver);
    let reporter = InMemoryReporter::default();

    assert!(service
        .optimize_with_reporter(valid_request(), &reporter)
        .is_err());

    let events = reporter.events();
    assert_eq!(
        events.last(),
        Some(&RecordedEvent::Error(
            OptimizationErrorCode::NoFeasibleRoute
        ))
    );
    assert!(events.iter().rev().skip(1).any(|event| matches!(
        event,
        RecordedEvent::Progress(ProgressStage::OptimizingRoute, _)
    )));
}
