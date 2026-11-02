#[test]
fn incumbent_exchange_is_rejected_until_real_lkh_benchmark_evidence_exists() {
    let config = SolverOrchestratorConfig {
        enable_mip_scip: true,
        enable_lkh3: true,
        lkh3_executable: Some("LKH".into()),
        incumbent_exchange: true,
        ..SolverOrchestratorConfig::default()
    };

    let error = config.validate().unwrap_err();
    assert!(error.to_string().contains("real LKH-3 benchmark"));
}

#[test]
fn selects_best_candidate_and_continues_after_failure() {
    let strategies: Vec<Arc<dyn OrchestratedStrategy>> = vec![
        Arc::new(FixedStrategy {
            name: "failure",
            route: None,
            fail: true,
            wait_for_cancellation: false,
        }),
        Arc::new(FixedStrategy {
            name: "heuristic",
            route: Some(vec![0, 2, 1, 3]),
            fail: false,
            wait_for_cancellation: false,
        }),
        Arc::new(FixedStrategy {
            name: "exact_bit_dp",
            route: Some(vec![0, 1, 2, 3]),
            fail: false,
            wait_for_cancellation: false,
        }),
    ];
    let solver =
        SolverOrchestrator::with_strategies(test_config(), DefaultObjectivePolicy, strategies)
            .unwrap();
    let problem = problem(4);
    let matrix = matrix(4);
    let cancellation = CancellationToken::new();
    let result = solver
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();

    assert_eq!(result.selected_strategy, "exact_bit_dp");
    assert_eq!(result.solution.visit_order, vec![0, 1, 2, 3]);
    assert_eq!(result.candidates.len(), 3);
    assert!(result
        .candidates
        .iter()
        .find(|candidate| candidate.strategy == "failure")
        .unwrap()
        .metadata
        .error
        .is_some());
}

#[test]
fn stable_tie_break_keeps_registration_order() {
    let metrics = SolutionMetrics {
        start_policy: crate::domain::StartPolicy::Latest,
        visited_optional_count: 0,
        visited_optional_priority: 0,
        start_time_slot: 10,
        finish_time_slot: 20,
        travel_minutes: 30,
        wait_minutes: 40,
        score: 70,
    };
    let candidate = |strategy: &str| SolverCandidate {
        strategy: strategy.to_owned(),
        route: Some(SolverSolution {
            visit_order: vec![0, 1],
        }),
        feasible: true,
        objective_score: Some(metrics),
        elapsed: Duration::ZERO,
        metadata: SolverCandidateMetadata::default(),
    };
    let mut candidates = vec![candidate("first"), candidate("second")];
    let evaluator = DefaultObjectivePolicy;
    let selector = CandidateSelector::new(&evaluator);
    selector.rank(&mut candidates);
    assert_eq!(selector.select(&candidates).unwrap().strategy, "first");
}

#[test]
fn selector_uses_latest_start_finish_travel_then_wait() {
    let candidate = |strategy: &str, metrics: SolutionMetrics| SolverCandidate {
        strategy: strategy.to_owned(),
        route: Some(SolverSolution {
            visit_order: vec![0, 1],
        }),
        feasible: true,
        objective_score: Some(metrics),
        elapsed: Duration::ZERO,
        metadata: SolverCandidateMetadata::default(),
    };
    let mut candidates = vec![
        candidate(
            "more_wait",
            SolutionMetrics {
                start_policy: crate::domain::StartPolicy::Latest,
                visited_optional_count: 0,
                visited_optional_priority: 0,
                start_time_slot: 20,
                finish_time_slot: 30,
                travel_minutes: 10,
                wait_minutes: 2,
                score: 12,
            },
        ),
        candidate(
            "less_travel",
            SolutionMetrics {
                start_policy: crate::domain::StartPolicy::Latest,
                visited_optional_count: 0,
                visited_optional_priority: 0,
                start_time_slot: 20,
                finish_time_slot: 30,
                travel_minutes: 9,
                wait_minutes: 100,
                score: 109,
            },
        ),
        candidate(
            "earlier_start",
            SolutionMetrics {
                start_policy: crate::domain::StartPolicy::Latest,
                visited_optional_count: 0,
                visited_optional_priority: 0,
                start_time_slot: 19,
                finish_time_slot: 20,
                travel_minutes: 1,
                wait_minutes: 0,
                score: 1,
            },
        ),
    ];
    let evaluator = DefaultObjectivePolicy;
    CandidateSelector::new(&evaluator).rank(&mut candidates);
    assert_eq!(candidates[0].strategy, "less_travel");
    assert_eq!(candidates[1].strategy, "more_wait");
    assert_eq!(candidates[2].strategy, "earlier_start");
}

#[test]
fn selector_applies_earliest_and_fixed_start_policies() {
    let evaluator = DefaultObjectivePolicy;
    let metrics = |policy, start_time_slot, finish_time_slot| SolutionMetrics {
        start_policy: policy,
        visited_optional_count: 0,
        visited_optional_priority: 0,
        start_time_slot,
        finish_time_slot,
        travel_minutes: 10,
        wait_minutes: 0,
        score: 10,
    };

    assert!(evaluator
        .compare(
            &metrics(crate::domain::StartPolicy::Earliest, 10, 30),
            &metrics(crate::domain::StartPolicy::Earliest, 11, 20),
        )
        .is_lt());
    assert!(evaluator
        .compare(
            &metrics(crate::domain::StartPolicy::Fixed, 10, 20),
            &metrics(crate::domain::StartPolicy::Fixed, 10, 21),
        )
        .is_lt());
}
