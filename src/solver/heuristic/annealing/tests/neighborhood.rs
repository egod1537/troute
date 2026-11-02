#[test]
fn neighborhood_operations_preserve_endpoints_and_permutation() {
    let original = vec![0, 1, 2, 3, 4];

    let mut swapped = original.clone();
    assert!(apply_neighborhood_move(
        &mut swapped,
        NeighborhoodMove::Swap { left: 1, right: 3 }
    ));
    assert_eq!(swapped, vec![0, 3, 2, 1, 4]);

    let mut relocated = original.clone();
    assert!(apply_neighborhood_move(
        &mut relocated,
        NeighborhoodMove::Relocate { from: 1, to: 3 }
    ));
    assert_eq!(relocated, vec![0, 2, 3, 1, 4]);

    let mut reversed = original.clone();
    assert!(apply_neighborhood_move(
        &mut reversed,
        NeighborhoodMove::TwoOpt { start: 1, end: 3 }
    ));
    assert_eq!(reversed, vec![0, 3, 2, 1, 4]);
}

#[test]
fn fixed_seed_is_deterministic() {
    let problem = problem(7);
    let matrix = directed_matrix(7);
    let solve = || {
        SimulatedAnnealingSolver::with_config(deterministic_config(250))
            .unwrap()
            .solve_detailed(SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            })
            .unwrap()
    };
    let first = solve();
    let second = solve();

    assert_eq!(first.solution, second.solution);
    assert_eq!(first.initial_solution, second.initial_solution);
    assert_eq!(first.initial_metrics, second.initial_metrics);
    assert_eq!(first.metrics, second.metrics);
    assert_eq!(first.stats.iterations, second.stats.iterations);
    assert_eq!(first.stats.accepted_moves, second.stats.accepted_moves);
    assert_eq!(first.stats.improved_moves, second.stats.improved_moves);
    assert_eq!(
        first.stats.swap_moves + first.stats.relocate_moves + first.stats.two_opt_moves,
        first.stats.iterations
    );
    assert_eq!(first.stats.initial_temperature, 1_000.0);
    assert_eq!(first.stats.cooling_rate, 0.99);
    assert!(first.stats.final_temperature < first.stats.initial_temperature);
    assert_eq!(
        first.stats.accepted_infeasible_moves,
        second.stats.accepted_infeasible_moves
    );
}

#[test]
fn mst_double_tree_generator_can_seed_annealing() {
    let problem = problem(7);
    let matrix = directed_matrix(7);
    let cancellation = CancellationToken::new();
    let result = SimulatedAnnealingSolver::new(
        deterministic_config(50),
        MstDoubleTreeInitialRoute::default(),
        MixedNeighborhoodStrategy,
        DefaultObjectivePolicy,
    )
    .unwrap()
    .solve_detailed(SolverInput {
        matrix: &matrix,
        problem: &problem,
        cancellation: &cancellation,
    })
    .unwrap();

    assert_eq!(result.solution.visit_order.len(), 7);
    assert!(evaluate_solution(
        &DefaultObjectivePolicy,
        SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &cancellation,
        },
        &result.solution,
    )
    .is_ok());
}

#[test]
fn worse_move_acceptance_is_temperature_and_probability_based() {
    assert!(simulated_annealing_accept(0.0, 1.0, 0.99));
    assert!(simulated_annealing_accept(-1.0, 0.0, 0.99));
    assert!(simulated_annealing_accept(10.0, 100.0, 0.5));
    assert!(!simulated_annealing_accept(10.0, 100.0, 0.99));
    assert!(!simulated_annealing_accept(10.0, 0.0, 0.0));
}
