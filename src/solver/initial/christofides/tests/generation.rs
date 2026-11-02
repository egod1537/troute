#[test]
fn christofides_generates_a_complete_route_with_forced_blossom() {
    let problem = problem(12);
    let matrix = line_metric(12);
    let cancellation = CancellationToken::new();
    let result = ChristofidesInitialRoute::new(AverageSymmetricDistance, BlossomPerfectMatching)
        .generate_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();

    let visited: BTreeSet<_> = result.solution.visit_order.iter().copied().collect();
    assert_eq!(visited, (0..12).collect());
    assert_eq!(result.solution.visit_order.first(), Some(&0));
    assert_eq!(result.solution.visit_order.last(), Some(&11));
    assert_eq!(result.matching_strategy, "blossom");
}

#[test]
fn mst_plus_matching_is_eulerian_and_shortcuts_every_vertex_once() {
    let problem = problem(6);
    let matrix = line_metric(6);
    let cancellation = CancellationToken::new();
    let result = ChristofidesInitialRoute::default()
        .generate_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();

    let odd_set: BTreeSet<_> = result.odd_vertices.iter().copied().collect();
    let matched: BTreeSet<_> = result
        .matching_edges
        .iter()
        .flat_map(|edge| [edge.left, edge.right])
        .collect();
    assert_eq!(odd_set, matched);
    assert_eq!(result.symmetric_distance_strategy, "average_bidirectional");
    assert_eq!(result.matching_strategy, "bit_dp");
    assert!(odd_degree_vertices(6, &result.eulerian_edges)
        .unwrap()
        .is_empty());
    assert_eq!(result.euler_tour.len(), result.eulerian_edges.len() + 1);
    let visited: BTreeSet<_> = result.solution.visit_order.iter().copied().collect();
    assert_eq!(visited, (0..6).collect());
    assert_eq!(result.solution.visit_order.len(), 6);
}

#[test]
fn small_metric_case_is_within_three_halves_of_exact_optimum() {
    let problem = problem(7);
    let matrix = line_metric(7);
    let cancellation = CancellationToken::new();
    let generated = ChristofidesInitialRoute::default()
        .generate_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();
    let generated_metrics = evaluate_solution(
        &DefaultObjectivePolicy,
        input(&problem, &matrix, &cancellation),
        &generated.solution,
    )
    .unwrap();
    let optimum = ExactBitDpSolver::default()
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();

    assert!(generated_metrics.travel_minutes * 2 <= optimum.metrics.travel_minutes * 3);
}

#[test]
fn final_evaluation_uses_the_original_directed_matrix() {
    let problem = problem(4);
    let matrix = TravelTimeMatrix::new(vec![
        vec![0, 1, 50, 50],
        vec![99, 0, 2, 50],
        vec![50, 98, 0, 3],
        vec![50, 50, 97, 0],
    ])
    .unwrap();
    let cancellation = CancellationToken::new();
    let generated = ChristofidesInitialRoute::default()
        .generate_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();
    let evaluated = evaluate_solution(
        &DefaultObjectivePolicy,
        input(&problem, &matrix, &cancellation),
        &generated.solution,
    )
    .unwrap();
    let directed_sum: u32 = generated
        .solution
        .visit_order
        .windows(2)
        .map(|edge| matrix.travel_minutes(edge[0], edge[1]).unwrap())
        .sum();
    let symmetric_sum: u64 = generated
        .solution
        .visit_order
        .windows(2)
        .map(|edge| {
            AverageSymmetricDistance
                .symmetric_distance(&matrix, edge[0], edge[1])
                .unwrap()
        })
        .sum();

    assert_eq!(evaluated.travel_minutes, directed_sum);
    assert_ne!(u64::from(evaluated.travel_minutes), symmetric_sum);
}

#[test]
fn generator_can_seed_simulated_annealing() {
    let problem = problem(7);
    let matrix = line_metric(7);
    let cancellation = CancellationToken::new();
    let solver = SimulatedAnnealingSolver::new(
        SimulatedAnnealingConfig {
            iteration_limit: Some(50),
            seed: Some(42),
            ..SimulatedAnnealingConfig::default()
        },
        ChristofidesInitialRoute::default(),
        MixedNeighborhoodStrategy,
        DefaultObjectivePolicy,
    )
    .unwrap();
    let result = solver
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();

    assert_eq!(result.solution.visit_order.first(), Some(&0));
    assert_eq!(result.solution.visit_order.last(), Some(&6));
}

#[test]
fn generation_is_deterministic() {
    let problem = problem(8);
    let matrix = line_metric(8);
    let cancellation = CancellationToken::new();
    let generator = ChristofidesInitialRoute::default();
    let first = generator
        .generate(
            input(&problem, &matrix, &cancellation),
            &mut StdRng::seed_from_u64(1),
        )
        .unwrap();
    let second = generator
        .generate(
            input(&problem, &matrix, &cancellation),
            &mut StdRng::seed_from_u64(999),
        )
        .unwrap();
    assert_eq!(first, second);
}
