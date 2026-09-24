use std::{collections::BTreeSet, time::Duration};

use serde_json::Value;
use troute::{
    domain::{Location, OptimizationProblem},
    matrix::TravelTimeMatrix,
    routing::{RoutingContext, RoutingError, RoutingProvider},
    solver::{
        ExactBitDpSolver, SimulatedAnnealingConfig, SimulatedAnnealingSolver, SolutionMetrics,
        SolverInput, SolverOrchestrator, SolverOrchestratorConfig,
    },
    CancellationToken, OptimizeRouteRequest, RouteOptimizationService,
};

const LOCATION_COUNT: usize = 5;
const SA_ITERATIONS: u64 = 250;

fn request() -> OptimizeRouteRequest {
    serde_json::from_value(serde_json::json!({
        "job_id": "refactor-regression",
        "locations": (0..LOCATION_COUNT)
            .map(|index| serde_json::json!({
                "id": format!("L{index}"),
                "place_id": format!("place-{index}"),
                "open_time": "00:00",
                "close_time": "23:50",
                "stay_minutes": 0
            }))
            .collect::<Vec<_>>(),
        "start_time": "09:00",
        "travel_time_matrix": (0..LOCATION_COUNT)
            .map(|from| (0..LOCATION_COUNT)
                .map(|to| if from == to { 0 } else { 10 })
                .collect::<Vec<_>>())
            .collect::<Vec<_>>()
    }))
    .unwrap()
}

fn case() -> (OptimizationProblem, TravelTimeMatrix) {
    let request = request();
    let matrix = TravelTimeMatrix::new(request.travel_time_matrix.clone().unwrap()).unwrap();
    let problem = request.try_into().unwrap();
    (problem, matrix)
}

fn input<'a>(
    problem: &'a OptimizationProblem,
    matrix: &'a TravelTimeMatrix,
    cancellation: &'a CancellationToken,
) -> SolverInput<'a> {
    SolverInput {
        problem,
        matrix,
        cancellation,
    }
}

fn expected_metrics() -> SolutionMetrics {
    SolutionMetrics {
        start_time_slot: 139,
        finish_time_slot: 143,
        travel_minutes: 40,
        wait_minutes: 0,
        score: 40,
    }
}

fn sa_config() -> SimulatedAnnealingConfig {
    SimulatedAnnealingConfig {
        iteration_limit: Some(SA_ITERATIONS),
        time_limit_ms: None,
        seed: Some(42),
        ..SimulatedAnnealingConfig::default()
    }
}

fn orchestrator_config() -> SolverOrchestratorConfig {
    SolverOrchestratorConfig {
        max_concurrency: 1,
        strategy_timeout: Duration::from_secs(5),
        sa_config: SimulatedAnnealingConfig {
            seed: None,
            ..sa_config()
        },
        sa_seeds: vec![42],
        ..SolverOrchestratorConfig::default()
    }
}

#[test]
fn exact_and_seeded_annealing_match_the_locked_baseline() {
    let (problem, matrix) = case();
    let cancellation = CancellationToken::new();
    let exact = ExactBitDpSolver::default()
        .solve_detailed(input(&problem, &matrix, &cancellation))
        .unwrap();

    assert_eq!(exact.solution.visit_order, vec![0, 3, 2, 1, 4]);
    assert_eq!(exact.metrics, expected_metrics());

    let solve_sa = || {
        SimulatedAnnealingSolver::with_config(sa_config())
            .unwrap()
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap()
    };
    let first = solve_sa();
    let second = solve_sa();

    assert_eq!(first.initial_solution.visit_order, vec![0, 3, 2, 1, 4]);
    assert_eq!(first.solution, second.solution);
    assert_eq!(first.metrics, expected_metrics());
    assert_eq!(first.metrics, second.metrics);
    assert_eq!(first.stats.iterations, SA_ITERATIONS);
    assert_eq!(first.stats.accepted_moves, second.stats.accepted_moves);
    assert_eq!(first.stats.improved_moves, second.stats.improved_moves);
    assert_eq!(first.stats.swap_moves, second.stats.swap_moves);
    assert_eq!(first.stats.relocate_moves, second.stats.relocate_moves);
    assert_eq!(first.stats.two_opt_moves, second.stats.two_opt_moves);
}

#[test]
fn orchestrator_selection_candidates_metrics_and_metadata_are_stable() {
    let (problem, matrix) = case();
    let cancellation = CancellationToken::new();
    let solve = || {
        SolverOrchestrator::new(orchestrator_config())
            .unwrap()
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap()
    };
    let first = solve();
    let second = solve();

    let expected_strategies = ["exact_bit_dp"];
    assert_eq!(first.selected_strategy, "exact_bit_dp");
    assert_eq!(first.solution.visit_order, vec![0, 3, 2, 1, 4]);
    assert_eq!(first.candidates.len(), expected_strategies.len());
    assert_eq!(
        first
            .candidates
            .iter()
            .map(|candidate| candidate.strategy.as_str())
            .collect::<Vec<_>>(),
        expected_strategies
    );
    assert!(first.candidates.iter().all(|candidate| {
        candidate.feasible && candidate.objective_score == Some(expected_metrics())
    }));

    let stable_candidate_fields = |result: &troute::solver::OrchestratorSolveResult| {
        result
            .candidates
            .iter()
            .map(|candidate| {
                (
                    candidate.strategy.clone(),
                    candidate.route.clone(),
                    candidate.feasible,
                    candidate.objective_score,
                    candidate.metadata.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        stable_candidate_fields(&first),
        stable_candidate_fields(&second)
    );
}

struct MustNotRoute;

impl RoutingProvider for MustNotRoute {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
        _context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        panic!("caller-supplied matrix must bypass the routing provider")
    }
}

fn keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .expect("contract value must be a JSON object")
        .keys()
        .map(String::as_str)
        .collect()
}

#[test]
fn caller_matrix_bypass_and_api_response_contract_are_stable() {
    let response = RouteOptimizationService::new(
        MustNotRoute,
        SolverOrchestrator::new(orchestrator_config()).unwrap(),
    )
    .optimize(request())
    .unwrap();
    let json = serde_json::to_value(response).unwrap();

    assert_eq!(
        keys(&json),
        BTreeSet::from([
            "route",
            "solver_candidates",
            "solver_diagnostics",
            "total_travel_minutes"
        ])
    );
    assert_eq!(json["total_travel_minutes"], 40);
    assert_eq!(
        json["route"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stop| stop["location_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["L0", "L3", "L2", "L1", "L4"]
    );
    assert_eq!(
        keys(&json["route"][0]),
        BTreeSet::from(["arrival_time", "departure_time", "location_id", "order"])
    );
    assert_eq!(
        keys(&json["route"][4]),
        BTreeSet::from(["arrival_time", "departure_time", "location_id", "order"])
    );

    let candidates = json["solver_candidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(
        candidates
            .iter()
            .filter(|candidate| candidate["best"] == true)
            .count(),
        1
    );
    let exact = candidates
        .iter()
        .find(|candidate| candidate["strategy"] == "exact_bit_dp")
        .unwrap();
    assert_eq!(
        keys(exact),
        BTreeSet::from([
            "best",
            "elapsed_ms",
            "feasible",
            "metadata",
            "objective_score",
            "route",
            "strategy"
        ])
    );
    assert_eq!(
        keys(&exact["objective_score"]),
        BTreeSet::from([
            "finish_time",
            "latest_start",
            "travel_minutes",
            "wait_minutes"
        ])
    );
    assert_eq!(exact["objective_score"]["latest_start"], "23:10");
    assert_eq!(exact["objective_score"]["finish_time"], "23:50");
    assert_eq!(exact["objective_score"]["travel_minutes"], 40);
    assert_eq!(
        keys(&exact["metadata"]),
        BTreeSet::from([
            "frontier_cell_count",
            "frontier_state_count",
            "state_count",
            "timed_out"
        ])
    );
}
