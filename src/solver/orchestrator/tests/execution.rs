#[test]
fn timeout_is_recorded_without_stopping_other_strategies() {
    let mut config = test_config();
    config.strategy_timeout = Duration::from_millis(10);
    let strategies: Vec<Arc<dyn OrchestratedStrategy>> = vec![
        Arc::new(FixedStrategy {
            name: "slow",
            route: None,
            fail: false,
            wait_for_cancellation: true,
        }),
        Arc::new(FixedStrategy {
            name: "fast",
            route: Some(vec![0, 1, 2]),
            fail: false,
            wait_for_cancellation: false,
        }),
    ];
    let solver =
        SolverOrchestrator::with_strategies(config, DefaultObjectivePolicy, strategies).unwrap();
    let problem = problem(3);
    let matrix = matrix(3);
    let cancellation = CancellationToken::new();
    let result = solver
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();

    assert_eq!(result.selected_strategy, "fast");
    let timed_out = result
        .candidates
        .iter()
        .find(|candidate| candidate.strategy == "slow")
        .unwrap();
    assert!(timed_out.metadata.timed_out);
}

#[test]
fn max_concurrency_bounds_simultaneous_strategies() {
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let strategies: Vec<Arc<dyn OrchestratedStrategy>> = (0..4)
        .map(|index| {
            Arc::new(ConcurrencyStrategy {
                name: format!("parallel-{index}"),
                active: Arc::clone(&active),
                peak: Arc::clone(&peak),
            }) as Arc<dyn OrchestratedStrategy>
        })
        .collect();
    let mut config = test_config();
    config.max_concurrency = 2;
    let solver =
        SolverOrchestrator::with_strategies(config, DefaultObjectivePolicy, strategies).unwrap();
    let problem = problem(2);
    let matrix = matrix(2);
    let cancellation = CancellationToken::new();

    solver
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();
    assert_eq!(peak.load(TestOrdering::SeqCst), 2);
}

#[test]
fn reports_no_feasible_candidate() {
    let strategies: Vec<Arc<dyn OrchestratedStrategy>> = vec![Arc::new(FixedStrategy {
        name: "invalid",
        route: Some(vec![0, 0, 2]),
        fail: false,
        wait_for_cancellation: false,
    })];
    let solver =
        SolverOrchestrator::with_strategies(test_config(), DefaultObjectivePolicy, strategies)
            .unwrap();
    let problem = problem(3);
    let matrix = matrix(3);
    let cancellation = CancellationToken::new();
    assert!(matches!(
        solver.solve_detailed(input(&problem, &matrix, &cancellation)),
        Err(SolverError::NoFeasibleRoute)
    ));
}

#[test]
fn exact_success_terminates_before_baselines_and_sa() {
    let mut config = test_config();
    config.sa_seeds = vec![7, 11];
    let solver = SolverOrchestrator::new(config).unwrap();
    let problem = problem(5);
    let matrix = matrix(5);
    let cancellation = CancellationToken::new();
    let first = solver
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();
    assert_eq!(first.selected_strategy, "exact_bit_dp");
    assert_eq!(first.candidates.len(), 1);
    assert_eq!(first.sa_run_count, 0);
    assert_eq!(first.termination_reason, "exact_optimum");
}

#[test]
fn omits_exact_above_configured_limit() {
    let mut config = test_config();
    config.exact_limit = 3;
    let solver = SolverOrchestrator::new(config).unwrap();
    let problem = problem(4);
    let matrix = matrix(4);
    let cancellation = CancellationToken::new();
    let result = solver
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();
    assert!(!result
        .candidates
        .iter()
        .any(|candidate| candidate.strategy == "exact_bit_dp"));
}
