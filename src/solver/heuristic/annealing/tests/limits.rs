#[derive(Default)]
struct SlowSwap {
    calls: Mutex<u64>,
}

impl NeighborhoodStrategy for SlowSwap {
    fn neighbor(
        &self,
        route: &[usize],
        _rng: &mut dyn RngCore,
    ) -> Option<(Vec<usize>, NeighborhoodMove)> {
        thread::sleep(Duration::from_millis(2));
        *self.calls.lock().unwrap() += 1;
        AlwaysSwap.neighbor(route, &mut StdRng::seed_from_u64(1))
    }
}

#[test]
fn iteration_and_time_limits_are_honored() {
    let problem = problem(5);
    let matrix = directed_matrix(5);
    let iteration_result = SimulatedAnnealingSolver::new(
        deterministic_config(17),
        FixedInitial(vec![0, 1, 2, 3, 4]),
        AlwaysSwap,
        DefaultObjectivePolicy,
    )
    .unwrap()
    .solve_detailed(SolverInput {
        matrix: &matrix,
        problem: &problem,
        cancellation: &CancellationToken::new(),
    })
    .unwrap();
    assert_eq!(iteration_result.stats.iterations, 17);

    let time_config = SimulatedAnnealingConfig {
        iteration_limit: Some(1_000_000),
        time_limit_ms: Some(5),
        ..deterministic_config(1_000_000)
    };
    let timed = SimulatedAnnealingSolver::new(
        time_config,
        FixedInitial(vec![0, 1, 2, 3, 4]),
        SlowSwap::default(),
        DefaultObjectivePolicy,
    )
    .unwrap()
    .solve_detailed(SolverInput {
        matrix: &matrix,
        problem: &problem,
        cancellation: &CancellationToken::new(),
    })
    .unwrap();
    assert!(timed.stats.iterations < 1_000_000);
    assert!(timed.stats.elapsed < Duration::from_millis(200));
}

#[test]
fn feasible_penalty_evaluation_reuses_shared_objective_metrics() {
    let problem = problem(5);
    let matrix = directed_matrix(5);
    let solution = SolverSolution {
        visit_order: vec![0, 1, 2, 3, 4],
    };
    let shared = evaluate_solution(
        &DefaultObjectivePolicy,
        SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        },
        &solution,
    )
    .unwrap();
    let penalized = evaluate_solution_with_penalties(
        &DefaultObjectivePolicy,
        SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        },
        &solution,
        InfeasiblePenaltyWeights::default(),
    )
    .unwrap();

    assert!(penalized.feasible);
    assert_eq!(penalized.feasible_metrics, Some(shared));
}
