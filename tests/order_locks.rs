use rand::{rngs::StdRng, Rng, SeedableRng};
use troute::{
    api::OptimizeRouteRequest,
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    schedule::calculate_schedule,
    solver::{
        evaluate_solution, DefaultObjectivePolicy, ExactBitDpSolver, GreedyInitialRoute,
        InitialRouteGenerator, ObjectivePolicy, RouteSolver, SimulatedAnnealingConfig,
        SimulatedAnnealingSolver, SolverError, SolverInput, SolverSolution,
    },
};

fn request(count: usize, locks: &[usize], partial: bool) -> OptimizeRouteRequest {
    serde_json::from_value(serde_json::json!({
        "job_id": "order-lock-test",
        "start_policy": "FIXED",
        "start_time": "09:00",
        "visit_policy": if partial { "BEST_EFFORT_ON_FAILURE" } else { "ALL_REQUIRED" },
        "locations": (0..count).map(|i| serde_json::json!({
            "id": i.to_string(), "place_id": format!("place-{i}"),
            "open_time": "00:00", "close_time": "23:59", "stay_minutes": 0,
            "order_locked": locks.contains(&i),
        })).collect::<Vec<_>>()
    }))
    .unwrap()
}

fn permutations(values: &mut [usize], offset: usize, visit: &mut impl FnMut(&[usize])) {
    if offset == values.len() {
        visit(values);
        return;
    }
    for index in offset..values.len() {
        values.swap(offset, index);
        permutations(values, offset + 1, visit);
        values.swap(offset, index);
    }
}

#[test]
fn exact_matches_brute_force_with_multiple_fixed_positions() {
    let problem: OptimizationProblem = request(7, &[2, 4], false).try_into().unwrap();
    let cancellation = CancellationToken::new();
    let mut rng = StdRng::seed_from_u64(91);
    for _ in 0..12 {
        let rows = (0..7)
            .map(|i| {
                (0..7)
                    .map(|j| if i == j { 0 } else { rng.gen_range(1..100) })
                    .collect()
            })
            .collect();
        let matrix = TravelTimeMatrix::new(rows).unwrap();
        let input = SolverInput {
            problem: &problem,
            matrix: &matrix,
            cancellation: &cancellation,
        };
        let solution = ExactBitDpSolver::default().solve(input).unwrap();
        assert_eq!(solution.visit_order[2], 2);
        assert_eq!(solution.visit_order[4], 4);
        let actual = evaluate_solution(&DefaultObjectivePolicy, input, &solution).unwrap();
        let mut best = None;
        permutations(&mut [1, 2, 3, 4, 5], 0, &mut |middle| {
            if middle[1] != 2 || middle[3] != 4 {
                return;
            }
            let order = std::iter::once(0)
                .chain(middle.iter().copied())
                .chain(std::iter::once(6))
                .collect();
            let metrics = evaluate_solution(
                &DefaultObjectivePolicy,
                input,
                &SolverSolution { visit_order: order },
            )
            .unwrap();
            if best.as_ref().is_none_or(|current| {
                ObjectivePolicy::compare(&DefaultObjectivePolicy, &metrics, current).is_lt()
            }) {
                best = Some(metrics);
            }
        });
        assert_eq!(actual, best.unwrap());
    }
}

#[test]
fn lock_conflicting_with_time_window_is_not_relaxed() {
    let mut request = request(4, &[2], false);
    request.locations[2].close_time = "09:10".parse().unwrap();
    let matrix = TravelTimeMatrix::new(vec![
        vec![0, 10, 10, 10],
        vec![10, 0, 10, 10],
        vec![10, 10, 0, 10],
        vec![10, 10, 10, 0],
    ])
    .unwrap();
    let cancellation = CancellationToken::new();
    let locked: OptimizationProblem = request.clone().try_into().unwrap();
    assert!(matches!(
        ExactBitDpSolver::default().solve(SolverInput {
            problem: &locked,
            matrix: &matrix,
            cancellation: &cancellation
        }),
        Err(SolverError::NoFeasibleRoute)
    ));
    request.locations[2].order_locked = false;
    let unlocked = request.try_into().unwrap();
    assert_eq!(
        ExactBitDpSolver::default()
            .solve(SolverInput {
                problem: &unlocked,
                matrix: &matrix,
                cancellation: &cancellation
            })
            .unwrap()
            .visit_order,
        vec![0, 2, 1, 3]
    );
}

#[test]
fn partial_solution_retains_lock_and_its_absolute_position() {
    let mut request = request(6, &[3], true);
    request.locations[4].close_time = "08:00".parse().unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    assert!(problem.locations()[3].required());
    let matrix = TravelTimeMatrix::new(vec![vec![0; 6]; 6]).unwrap();
    let cancellation = CancellationToken::new();
    let input = SolverInput {
        problem: &problem,
        matrix: &matrix,
        cancellation: &cancellation,
    };
    let solution = ExactBitDpSolver::default().solve(input).unwrap();
    assert_eq!(solution.visit_order.len(), 5);
    assert_eq!(solution.visit_order[3], 3);
    assert!(!solution.visit_order.contains(&4));
    calculate_schedule(&problem, &matrix, &solution).unwrap();
    let shifted = SolverSolution {
        visit_order: vec![0, 1, 3, 5],
    };
    assert!(evaluate_solution(&DefaultObjectivePolicy, input, &shifted).is_err());
    assert!(calculate_schedule(&problem, &matrix, &shifted).is_err());
}

#[test]
fn locked_location_cannot_be_marked_optional() {
    let mut request = request(4, &[1], true);
    request.locations[1].required = Some(false);
    assert!(OptimizationProblem::try_from(request).is_err());
}

#[test]
fn annealing_preserves_locks_above_exact_solver_limit() {
    let problem: OptimizationProblem = request(18, &[2, 7, 13], false).try_into().unwrap();
    let mut rng = StdRng::seed_from_u64(17);
    let rows = (0..18)
        .map(|i| {
            (0..18)
                .map(|j| if i == j { 0 } else { rng.gen_range(1..20) })
                .collect()
        })
        .collect();
    let matrix = TravelTimeMatrix::new(rows).unwrap();
    let cancellation = CancellationToken::new();
    let input = SolverInput {
        problem: &problem,
        matrix: &matrix,
        cancellation: &cancellation,
    };
    let seed = GreedyInitialRoute.generate(input, &mut rng).unwrap();
    assert!(problem.preserves_order_locks(&seed.visit_order));
    let solver = SimulatedAnnealingSolver::with_config(SimulatedAnnealingConfig {
        seed: Some(5),
        iteration_limit: Some(10_000),
        time_limit_ms: Some(1000),
        ..SimulatedAnnealingConfig::default()
    })
    .unwrap();
    let result = solver.solve_detailed(input).unwrap();
    assert!(problem.preserves_order_locks(&result.initial_solution.visit_order));
    assert!(problem.preserves_order_locks(&result.solution.visit_order));
    assert!(result.stats.accepted_moves > 0);
    calculate_schedule(&problem, &matrix, &result.solution).unwrap();
}

#[cfg(feature = "solver-mip-scip")]
#[test]
fn mip_uses_actual_partial_route_positions_without_rank_gaps() {
    use std::time::Duration;
    use troute::solver::{OrchestratedStrategy, ScipMipConfig, ScipMipStrategy};
    let mut request = request(6, &[3], true);
    request.locations[1].close_time = "08:00".parse().unwrap();
    let problem: OptimizationProblem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0; 6]; 6]).unwrap();
    let cancellation = CancellationToken::new();
    let input = SolverInput {
        problem: &problem,
        matrix: &matrix,
        cancellation: &cancellation,
    };
    let result = ScipMipStrategy::new(ScipMipConfig {
        time_limit: Duration::from_secs(5),
        threads: 1,
        memory_mb: 512,
        seed: 42,
    })
    .unwrap()
    .execute(input)
    .unwrap();
    assert_eq!(result.solution.visit_order.len(), 5);
    assert_eq!(result.solution.visit_order[3], 3);
    assert!(!result.solution.visit_order.contains(&1));
    evaluate_solution(&DefaultObjectivePolicy, input, &result.solution).unwrap();
}
