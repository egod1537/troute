#[test]
fn success_reports_real_pipeline_boundaries_in_monotonic_order() {
    let service = RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver);
    let reporter = InMemoryReporter::default();

    let response = service
        .optimize_with_reporter(valid_request(), &reporter)
        .unwrap();

    let events = reporter.events();
    assert_eq!(
        events.last(),
        Some(&RecordedEvent::Result(Box::new(response)))
    );
    let progress: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RecordedEvent::Progress(stage, progress) => Some((*stage, *progress)),
            _ => None,
        })
        .collect();
    assert!(progress.windows(2).all(|pair| pair[0].1 <= pair[1].1));
    let mut stages: Vec<_> = progress.iter().map(|(stage, _)| *stage).collect();
    stages.dedup();
    assert_eq!(
        stages,
        [
            ProgressStage::Accepted,
            ProgressStage::ValidatingRequest,
            ProgressStage::SelectingProvider,
            ProgressStage::PreparingMatrix,
            ProgressStage::FetchingTravelTimes,
            ProgressStage::BuildingMatrix,
            ProgressStage::GeneratingCandidates,
            ProgressStage::OptimizingRoute,
            ProgressStage::SelectingBestCandidate,
            ProgressStage::Scheduling,
            ProgressStage::ValidatingSchedule,
            ProgressStage::FinalizingResult,
        ]
    );
    assert!(reporter.messages().iter().all(|message| message
        .as_deref()
        .is_some_and(|message| !message.is_empty())));
}

#[test]
fn exact_solver_start_selection_reaches_the_schedule() {
    let service =
        RouteOptimizationService::new(DevelopmentRoutingProvider, ExactBitDpSolver::default());
    let reporter = InMemoryReporter::default();
    let response = service
        .optimize_with_reporter(valid_request(), &reporter)
        .unwrap();

    assert_eq!(
        response.route[0].departure_time.unwrap().to_string(),
        "23:35"
    );
    assert_eq!(response.route[1].arrival_time.to_string(), "23:50");
    assert!(reporter.messages().iter().any(|message| {
        message
            .as_deref()
            .is_some_and(|message| message.contains("exact_bit_dp"))
    }));
}

#[test]
fn exact_solver_honors_all_start_policies() {
    let service =
        RouteOptimizationService::new(DevelopmentRoutingProvider, ExactBitDpSolver::default());

    let mut fixed = serde_json::to_value(valid_request()).unwrap();
    fixed["start_policy"] = serde_json::json!("FIXED");
    fixed["start_time"] = serde_json::json!("09:05");
    let fixed = service
        .optimize(serde_json::from_value(fixed).unwrap())
        .unwrap();
    assert_eq!(fixed.selected_start_time.unwrap().to_string(), "09:05");
    assert_eq!(fixed.route[0].departure_time.unwrap().to_string(), "09:05");

    let mut earliest = serde_json::to_value(valid_request()).unwrap();
    earliest["start_policy"] = serde_json::json!("EARLIEST");
    earliest.as_object_mut().unwrap().remove("start_time");
    let earliest = service
        .optimize(serde_json::from_value(earliest).unwrap())
        .unwrap();
    assert_eq!(earliest.selected_start_time.unwrap().to_string(), "00:00");

    let mut latest = serde_json::to_value(valid_request()).unwrap();
    latest["start_policy"] = serde_json::json!("LATEST");
    latest.as_object_mut().unwrap().remove("start_time");
    let latest = service
        .optimize(serde_json::from_value(latest).unwrap())
        .unwrap();
    assert_eq!(latest.selected_start_time.unwrap().to_string(), "23:35");
}

#[test]
fn best_effort_response_reports_actual_skipped_locations() {
    let service =
        RouteOptimizationService::new(DevelopmentRoutingProvider, ExactBitDpSolver::default());
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "partial-response",
            "visit_policy": "BEST_EFFORT_ON_FAILURE",
            "start_policy": "FIXED",
            "start_time": "09:00",
            "locations": [
                {"id":"start","place_id":"","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"skip","priority":100,"place_id":"","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"visit-a","priority":1,"place_id":"","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"visit-b","priority":5,"place_id":"","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"end","place_id":"","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ],
            "sparse_travel_time_matrix": [
                [0,1,1,null,20],
                [null,0,null,null,1],
                [null,null,0,1,10],
                [null,null,null,0,1],
                [null,null,null,null,0]
            ]
        }))
        .unwrap();

    let response = service.optimize(request).unwrap();
    assert_eq!(response.partial, Some(true));
    assert_eq!(response.visited_count, Some(4));
    assert_eq!(
        response
            .route
            .iter()
            .map(|stop| stop.location_id.as_str())
            .collect::<Vec<_>>(),
        ["start", "visit-a", "visit-b", "end"]
    );
    let skipped = response.skipped_locations.unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].location_id, "skip");
    assert_eq!(skipped[0].reason, "NOT_SELECTED_DUE_TO_CONSTRAINTS");
}

#[test]
fn best_candidate_objective_matches_the_returned_schedule() {
    let solver = SolverOrchestrator::new(SolverOrchestratorConfig::default()).unwrap();
    let service = RouteOptimizationService::new(DevelopmentRoutingProvider, solver);
    let response = service.optimize(valid_request()).unwrap();
    let last_departure = response.route.last().unwrap().departure_time.unwrap();
    let total_wait: u32 = response
        .route
        .iter()
        .map(|stop| stop.wait_minutes.unwrap())
        .sum();
    let best = response
        .solver_candidates
        .as_ref()
        .unwrap()
        .iter()
        .find(|candidate| candidate.best)
        .unwrap();
    let score = best.objective_score.unwrap();

    assert_eq!(best.route, ["A", "B"]);
    assert_eq!(score.start_time, response.selected_start_time);
    assert_eq!(score.finish_time, last_departure);
    assert_eq!(score.travel_minutes, response.total_travel_minutes);
    assert_eq!(score.wait_minutes, total_wait);
}

#[test]
fn orchestrator_diagnostics_reach_the_api_response() {
    let solver = SolverOrchestrator::new(SolverOrchestratorConfig {
        exact_limit: 1,
        total_budget: Duration::from_millis(30),
        sa_chunk: Duration::from_millis(5),
        deadline_safety_margin: Duration::from_millis(1),
        sa_config: SimulatedAnnealingConfig {
            iteration_limit: Some(1),
            ..SimulatedAnnealingConfig::default()
        },
        ..SolverOrchestratorConfig::default()
    })
    .unwrap();
    let service = RouteOptimizationService::new(DevelopmentRoutingProvider, solver);
    let response = service.optimize(valid_request()).unwrap();
    let candidates = response.solver_candidates.unwrap();

    assert!(!candidates
        .iter()
        .any(|candidate| candidate.strategy == "exact_bit_dp"));
    let clustered = candidates
        .iter()
        .find(|candidate| candidate.strategy == "clustered")
        .unwrap();
    assert_eq!(clustered.metadata.cluster_count, Some(0));
    assert_eq!(clustered.metadata.cluster_sizes, Some(Vec::new()));
    assert_eq!(
        clustered.metadata.cluster_strategy.as_deref(),
        Some("directed_nearest_neighbor")
    );
    assert_eq!(
        clustered.metadata.cluster_order_strategy.as_deref(),
        Some("greedy_bridge")
    );
    assert!(clustered.metadata.score_before_improvement.is_some());
    assert!(clustered.metadata.score_after_improvement.is_some());
    assert_eq!(clustered.metadata.swap_enabled, Some(true));
    assert_eq!(clustered.metadata.relocate_enabled, Some(false));
    assert_eq!(clustered.metadata.two_opt_enabled, Some(false));
    let mst = candidates
        .iter()
        .find(|candidate| candidate.strategy == "mst_double_tree")
        .unwrap();
    assert_eq!(
        mst.metadata.symmetric_distance_strategy.as_deref(),
        Some("average_bidirectional")
    );
    assert_eq!(mst.metadata.mst_edge_count, Some(1));
    assert_eq!(mst.metadata.mst_edges.as_ref().unwrap().len(), 1);
    assert_eq!(
        mst.metadata.euler_tour.as_ref().unwrap(),
        &vec!["A".to_owned(), "B".to_owned(), "A".to_owned()]
    );
    assert_eq!(
        mst.metadata.shortcut_route.as_ref().unwrap(),
        &vec!["A".to_owned(), "B".to_owned()]
    );
    let christofides = candidates
        .iter()
        .find(|candidate| candidate.strategy == "christofides")
        .unwrap();
    assert_eq!(christofides.metadata.mst_edge_count, Some(1));
    assert_eq!(christofides.metadata.odd_vertex_count, Some(2));
    assert_eq!(
        christofides.metadata.odd_vertices.as_ref().unwrap(),
        &vec!["A".to_owned(), "B".to_owned()]
    );
    assert_eq!(
        christofides.metadata.matching_strategy.as_deref(),
        Some("bit_dp")
    );
    assert_eq!(
        christofides.metadata.matching_pairs.as_ref().unwrap().len(),
        1
    );
    assert_eq!(
        christofides.metadata.shortcut_route.as_ref().unwrap(),
        &vec!["A".to_owned(), "B".to_owned()]
    );
    let sa_greedy = candidates
        .iter()
        .find(|candidate| candidate.strategy.starts_with("sa_greedy_"))
        .unwrap();
    assert_eq!(
        sa_greedy.metadata.initial_strategy.as_deref(),
        Some("greedy")
    );
    assert_eq!(
        sa_greedy.metadata.initial_route.as_ref().unwrap(),
        &vec!["A".to_owned(), "B".to_owned()]
    );
    assert_eq!(
        sa_greedy.metadata.final_route.as_ref().unwrap(),
        &vec!["A".to_owned(), "B".to_owned()]
    );
    assert_eq!(sa_greedy.metadata.iteration_count, Some(0));
    assert_eq!(sa_greedy.metadata.best_feasible, Some(true));
    assert_eq!(
        candidates.iter().filter(|candidate| candidate.best).count(),
        1
    );
    assert!(candidates
        .iter()
        .any(|candidate| candidate.strategy.starts_with("sa_greedy_")));
}
