use std::{hint::black_box, time::Instant};

use rand::{rngs::StdRng, SeedableRng};

use troute::{
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    solver::{
        evaluate_solution_with_penalties, AutoPerfectMatching, AverageSymmetricDistance,
        BitDpPerfectMatching, BlossomPerfectMatching, ChristofidesInitialRoute,
        ClusteredInitialRoute, ClusteredSolver, ClusteredSolverConfig, DefaultObjectivePolicy,
        ExactBitDpSolver, GreedyInitialRoute, InfeasiblePenaltyWeights, InitialRouteGenerator,
        MatchingStrategyConfig, MixedNeighborhoodStrategy, MstDoubleTreeInitialRoute,
        PerfectMatchingStrategy, SimulatedAnnealingConfig, SimulatedAnnealingSolver, SolverInput,
        SolverOrchestrator, SolverOrchestratorConfig,
    },
    OptimizeRouteRequest,
};

fn main() {
    benchmark_matching_strategies();
    benchmark_small(12);
    benchmark_large(40);
}

fn benchmark_matching_strategies() {
    println!("matching strategy,odd_count,matching_cost,elapsed_us,estimated_owned_bytes");
    let bit_dp = BitDpPerfectMatching::default();
    benchmark_matching(
        "bitdp",
        bit_dp,
        20,
        bit_dp.estimated_working_memory_bytes(20),
    );
    let blossom = BlossomPerfectMatching;
    benchmark_matching(
        "blossom",
        blossom,
        20,
        blossom.estimated_working_memory_bytes(20),
    );
    benchmark_matching(
        "blossom_large",
        blossom,
        64,
        blossom.estimated_working_memory_bytes(64),
    );
}

fn benchmark_matching<S: PerfectMatchingStrategy>(
    name: &str,
    strategy: S,
    odd_count: usize,
    estimated_memory: Option<usize>,
) {
    let vertices: Vec<_> = (0..odd_count).collect();
    let distances = matching_distances(odd_count);
    let started = Instant::now();
    let matching = strategy
        .minimum_weight_perfect_matching(&vertices, &distances)
        .unwrap();
    let elapsed = started.elapsed();
    let cost: u64 = matching.iter().map(|edge| edge.distance).sum();
    black_box(&matching);
    println!(
        "{name},{odd_count},{cost},{},{}",
        elapsed.as_micros(),
        estimated_memory.unwrap_or(0)
    );
}

fn benchmark_small(location_count: usize) {
    let problem = problem(location_count);
    let matrix = directed_matrix(location_count);
    let cancellation = CancellationToken::new();

    let exact_started = Instant::now();
    let exact = ExactBitDpSolver::default()
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();
    let exact_elapsed = exact_started.elapsed();
    black_box(&exact);

    let clustered_started = Instant::now();
    let clustered = ClusteredSolver::with_config(ClusteredSolverConfig::new(4).unwrap())
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();
    let clustered_elapsed = clustered_started.elapsed();
    black_box(&clustered);

    let annealing_started = Instant::now();
    let annealing = SimulatedAnnealingSolver::with_config(SimulatedAnnealingConfig {
        iteration_limit: Some(3_000),
        seed: Some(42),
        ..SimulatedAnnealingConfig::default()
    })
    .unwrap()
    .solve_detailed(input(&problem, &matrix, &cancellation))
    .unwrap();
    let annealing_elapsed = annealing_started.elapsed();
    black_box(&annealing);

    println!("small_n solver,n,elapsed_us,start_slot,finish_slot,score,states,frontier_states,clusters,iterations,accepted,improved,feasible");
    println!(
        "exact,{location_count},{},{},{},{},{},{},1,0,0,0,true",
        exact_elapsed.as_micros(),
        exact.metrics.start_time_slot,
        exact.metrics.finish_time_slot,
        exact.metrics.score,
        exact.stats.generated_states,
        exact.stats.frontier_states,
    );
    println!(
        "clustered,{location_count},{},{},{},{},{},{},{},0,0,{},true",
        clustered_elapsed.as_micros(),
        clustered.metrics.start_time_slot,
        clustered.metrics.finish_time_slot,
        clustered.metrics.score,
        clustered.stats.exact_generated_states,
        clustered.stats.exact_frontier_states,
        clustered.stats.cluster_count,
        clustered.stats.accepted_local_moves,
    );
    println!(
        "annealing,{location_count},{},{},{},{},0,0,0,{},{},{},true",
        annealing_elapsed.as_micros(),
        annealing.metrics.start_time_slot,
        annealing.metrics.finish_time_slot,
        annealing.metrics.score,
        annealing.stats.iterations,
        annealing.stats.accepted_moves,
        annealing.stats.improved_moves,
    );
    println!(
        "small_objective_gap solver=clustered,start_slots={},finish_slots={},score={} solver=annealing,start_slots={},finish_slots={},score={}",
        i32::from(exact.metrics.start_time_slot) - i32::from(clustered.metrics.start_time_slot),
        i32::from(clustered.metrics.finish_time_slot) - i32::from(exact.metrics.finish_time_slot),
        i128::from(clustered.metrics.score) - i128::from(exact.metrics.score),
        i32::from(exact.metrics.start_time_slot) - i32::from(annealing.metrics.start_time_slot),
        i32::from(annealing.metrics.finish_time_slot) - i32::from(exact.metrics.finish_time_slot),
        i128::from(annealing.metrics.score) - i128::from(exact.metrics.score),
    );

    println!(
        "initial_route solver,n,elapsed_us,initial_score,feasible,sa_final_score,sa_iterations"
    );
    benchmark_initializer(
        "greedy",
        GreedyInitialRoute,
        &problem,
        &matrix,
        &cancellation,
    );
    benchmark_initializer(
        "mst_double_tree",
        MstDoubleTreeInitialRoute::default(),
        &problem,
        &matrix,
        &cancellation,
    );
    benchmark_initializer(
        "christofides_bitdp",
        ChristofidesInitialRoute::new(AverageSymmetricDistance, BitDpPerfectMatching::default()),
        &problem,
        &matrix,
        &cancellation,
    );
    benchmark_initializer(
        "christofides_blossom",
        ChristofidesInitialRoute::new(AverageSymmetricDistance, BlossomPerfectMatching),
        &problem,
        &matrix,
        &cancellation,
    );
    let matching_config = MatchingStrategyConfig::from_env().unwrap();
    let configured_name = format!("christofides_{}", matching_config.strategy);
    benchmark_initializer(
        &configured_name,
        ChristofidesInitialRoute::new(
            AverageSymmetricDistance,
            AutoPerfectMatching::new(matching_config).unwrap(),
        ),
        &problem,
        &matrix,
        &cancellation,
    );
    benchmark_initializer(
        "clustered",
        ClusteredInitialRoute::new(ClusteredSolver::with_config(
            ClusteredSolverConfig::new(4).unwrap(),
        )),
        &problem,
        &matrix,
        &cancellation,
    );
    benchmark_orchestrator(&problem, &matrix, &cancellation, 3_000);
}

fn benchmark_initializer<G: InitialRouteGenerator>(
    name: &str,
    generator: G,
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    cancellation: &CancellationToken,
) {
    let mut rng = StdRng::seed_from_u64(42);
    let initial_started = Instant::now();
    let initial = generator
        .generate(input(problem, matrix, cancellation), &mut rng)
        .unwrap();
    let evaluated = evaluate_solution_with_penalties(
        &DefaultObjectivePolicy,
        input(problem, matrix, cancellation),
        &initial,
        InfeasiblePenaltyWeights::default(),
    )
    .unwrap();
    let initial_elapsed = initial_started.elapsed();
    let initial_score = evaluated
        .feasible_metrics
        .map(|metrics| metrics.score as f64)
        .unwrap_or(evaluated.energy);

    let annealed = SimulatedAnnealingSolver::new(
        SimulatedAnnealingConfig {
            iteration_limit: Some(3_000),
            seed: Some(42),
            ..SimulatedAnnealingConfig::default()
        },
        generator,
        MixedNeighborhoodStrategy,
        DefaultObjectivePolicy,
    )
    .unwrap()
    .solve_detailed(input(problem, matrix, cancellation))
    .unwrap();
    println!(
        "{name},{},{},{initial_score:.2},{},{},{}",
        problem.locations().len(),
        initial_elapsed.as_micros(),
        evaluated.feasible,
        annealed.metrics.score,
        annealed.stats.iterations
    );
}

fn benchmark_large(location_count: usize) {
    let problem = problem(location_count);
    let matrix = directed_matrix(location_count);
    let cancellation = CancellationToken::new();

    let clustered_started = Instant::now();
    let clustered = ClusteredSolver::default()
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();
    let clustered_elapsed = clustered_started.elapsed();

    let annealing_started = Instant::now();
    let annealing = SimulatedAnnealingSolver::with_config(SimulatedAnnealingConfig {
        iteration_limit: Some(5_000),
        seed: Some(42),
        ..SimulatedAnnealingConfig::default()
    })
    .unwrap()
    .solve_detailed(input(&problem, &matrix, &cancellation))
    .unwrap();
    let annealing_elapsed = annealing_started.elapsed();

    println!("large_n solver,n,elapsed_us,score,feasible,clusters,iterations,accepted,improved,accepted_infeasible");
    println!(
        "clustered,{location_count},{},{},true,{},0,0,{},0",
        clustered_elapsed.as_micros(),
        clustered.metrics.score,
        clustered.stats.cluster_count,
        clustered.stats.accepted_local_moves,
    );
    println!(
        "annealing,{location_count},{},{},true,0,{},{},{},{}",
        annealing_elapsed.as_micros(),
        annealing.metrics.score,
        annealing.stats.iterations,
        annealing.stats.accepted_moves,
        annealing.stats.improved_moves,
        annealing.stats.accepted_infeasible_moves,
    );
    benchmark_orchestrator(&problem, &matrix, &cancellation, 5_000);
}

fn benchmark_orchestrator(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    cancellation: &CancellationToken,
    iteration_limit: u64,
) {
    let solver = SolverOrchestrator::new(SolverOrchestratorConfig {
        max_cluster_size: 4,
        sa_config: SimulatedAnnealingConfig {
            iteration_limit: Some(iteration_limit),
            ..SimulatedAnnealingConfig::default()
        },
        ..SolverOrchestratorConfig::default()
    })
    .unwrap();
    let started = Instant::now();
    let result = solver
        .solve_detailed(input(problem, matrix, cancellation))
        .unwrap();
    let total_elapsed = started.elapsed();
    let exact = result
        .candidates
        .iter()
        .find(|candidate| candidate.strategy == "exact_bit_dp")
        .and_then(|candidate| candidate.objective_score);
    println!("orchestrator strategy,n,best,feasible,start_slot,finish_slot,travel,wait,elapsed_us,total_elapsed_us,start_gap,finish_gap,travel_gap,clusters,states,frontier_states,iterations,seed,error");
    for candidate in &result.candidates {
        let metrics = candidate.objective_score;
        let start_gap = exact.zip(metrics).map(|(exact, value)| {
            i32::from(exact.start_time_slot) - i32::from(value.start_time_slot)
        });
        let finish_gap = exact.zip(metrics).map(|(exact, value)| {
            i32::from(value.finish_time_slot) - i32::from(exact.finish_time_slot)
        });
        let travel_gap = exact.zip(metrics).map(|(exact, value)| {
            i64::from(value.travel_minutes) - i64::from(exact.travel_minutes)
        });
        println!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            candidate.strategy,
            problem.locations().len(),
            candidate.strategy == result.selected_strategy,
            candidate.feasible,
            metrics.map(|value| value.start_time_slot).unwrap_or(0),
            metrics.map(|value| value.finish_time_slot).unwrap_or(0),
            metrics.map(|value| value.travel_minutes).unwrap_or(0),
            metrics.map(|value| value.wait_minutes).unwrap_or(0),
            candidate.elapsed.as_micros(),
            total_elapsed.as_micros(),
            start_gap.unwrap_or(0),
            finish_gap.unwrap_or(0),
            travel_gap.unwrap_or(0),
            candidate.metadata.cluster_count.unwrap_or(0),
            candidate.metadata.state_count.unwrap_or(0),
            candidate.metadata.frontier_state_count.unwrap_or(0),
            candidate.metadata.iteration_count.unwrap_or(0),
            candidate.metadata.seed.unwrap_or(0),
            candidate.metadata.error.as_deref().unwrap_or("")
        );
    }
}

fn problem(location_count: usize) -> OptimizationProblem {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": format!("solver-benchmark-{location_count}"),
        "locations": (0..location_count)
            .map(|index| serde_json::json!({
                "id": index.to_string(),
                "place_id": format!("place-{index}"),
                "open_time": "09:00",
                "close_time": "23:50",
                "stay_minutes": if index == 0 || index + 1 == location_count { 0 } else { 10 }
            }))
            .collect::<Vec<_>>(),
        "start_time": "09:00"
    }))
    .unwrap();
    request.try_into().unwrap()
}

fn directed_matrix(location_count: usize) -> TravelTimeMatrix {
    TravelTimeMatrix::new(
        (0..location_count)
            .map(|from| {
                (0..location_count)
                    .map(|to| {
                        if from == to {
                            0
                        } else if to == from + 1 {
                            10
                        } else if to > from {
                            30 + (to - from) as u32
                        } else {
                            200 + (from - to) as u32
                        }
                    })
                    .collect()
            })
            .collect(),
    )
    .unwrap()
}

fn matching_distances(size: usize) -> Vec<Vec<u64>> {
    (0..size)
        .map(|left| {
            (0..size)
                .map(|right| {
                    if left == right {
                        0
                    } else {
                        left.abs_diff(right) as u64 * 7 + ((left + right) % 5) as u64
                    }
                })
                .collect()
        })
        .collect()
}

fn input<'a>(
    problem: &'a OptimizationProblem,
    matrix: &'a TravelTimeMatrix,
    cancellation: &'a CancellationToken,
) -> SolverInput<'a> {
    SolverInput {
        matrix,
        problem,
        cancellation,
    }
}
