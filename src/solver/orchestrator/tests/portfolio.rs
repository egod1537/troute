#[test]
fn large_problem_runs_all_initializers_then_warm_starts_until_deadline() {
    let mut config = test_config();
    config.exact_limit = 3;
    config.total_budget = Duration::from_millis(60);
    config.sa_chunk = Duration::from_millis(4);
    config.deadline_safety_margin = Duration::from_millis(2);
    config.sa_config.iteration_limit = Some(25);
    let solver = SolverOrchestrator::new(config).unwrap();
    let problem = problem(6);
    let matrix = matrix(6);
    let cancellation = CancellationToken::new();
    let result = solver
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();

    assert!(result.sa_run_count >= 5);
    for prefix in [
        "sa_greedy_run_1_",
        "sa_mst_run_2_",
        "sa_christofides_run_3_",
        "sa_clustered_run_4_",
        "sa_warm_start_run_5_",
    ] {
        assert!(result
            .candidates
            .iter()
            .any(|candidate| candidate.strategy.starts_with(prefix)));
    }
    let warm = result
        .candidates
        .iter()
        .find(|candidate| candidate.strategy.starts_with("sa_warm_start_run_5_"))
        .unwrap();
    let best_before_warm = result
        .candidates
        .iter()
        .filter(|candidate| {
            !candidate.strategy.starts_with("sa_")
                || (1..=4).any(|run| candidate.strategy.contains(&format!("_run_{run}_")))
        })
        .filter(|candidate| candidate.feasible)
        .min_by(|left, right| {
            DefaultObjectivePolicy.compare(
                left.objective_score.as_ref().unwrap(),
                right.objective_score.as_ref().unwrap(),
            )
        })
        .unwrap();
    let best_route_ids = best_before_warm
        .route
        .as_ref()
        .unwrap()
        .visit_order
        .iter()
        .map(|&index| problem.locations()[index].id().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(warm.metadata.initial_route.as_ref(), Some(&best_route_ids));
    assert_eq!(result.termination_reason, "deadline");
    assert!(result
        .candidates
        .iter()
        .any(|candidate| candidate.strategy == "greedy"));
}

#[test]
fn seed_sequence_is_deterministic_and_objective_never_selects_a_worse_result() {
    let mut config = test_config();
    config.exact_limit = 3;
    config.total_budget = Duration::from_millis(45);
    config.sa_chunk = Duration::from_millis(3);
    config.deadline_safety_margin = Duration::from_millis(2);
    config.sa_config.iteration_limit = Some(20);
    config.sa_seeds = vec![1234];
    let problem = problem(6);
    let matrix = matrix(6);
    let solve = || {
        let cancellation = CancellationToken::new();
        SolverOrchestrator::new(config.clone())
            .unwrap()
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap()
    };
    let first = solve();
    let second = solve();
    let first_four = |result: &OrchestratorSolveResult| {
        (1..=4)
            .map(|run| {
                result
                    .candidates
                    .iter()
                    .find(|candidate| candidate.strategy.contains(&format!("_run_{run}_")))
                    .and_then(|candidate| candidate.metadata.seed)
                    .unwrap()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(first_four(&first), first_four(&second));

    let selected = first.candidates[0].objective_score.unwrap();
    assert!(first
        .candidates
        .iter()
        .filter_map(|candidate| candidate.objective_score)
        .all(|metrics| !DefaultObjectivePolicy.compare(&selected, &metrics).is_gt()));
}

#[test]
fn cancellation_preserves_a_baseline_feasible_solution() {
    let mut config = test_config();
    config.exact_limit = 3;
    config.total_budget = Duration::from_millis(500);
    config.sa_chunk = Duration::from_millis(100);
    config.deadline_safety_margin = Duration::from_millis(2);
    config.sa_config.iteration_limit = None;
    let solver = SolverOrchestrator::new(config).unwrap();
    let problem = problem(8);
    let matrix = matrix(8);
    let cancellation = CancellationToken::new();
    let canceller = cancellation.clone();
    let handle = thread::spawn(move || {
        thread::sleep(Duration::from_millis(40));
        canceller.cancel();
    });
    let result = solver
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();
    handle.join().unwrap();

    assert_eq!(result.termination_reason, "cancellation");
    assert!(result.candidates[0].feasible);
}
