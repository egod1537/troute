use std::{sync::Mutex, thread, time::Duration};

use rand::{rngs::StdRng, RngCore, SeedableRng};

use crate::{
    api::OptimizeRouteRequest,
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    solver::{
        evaluate_solution, DefaultObjectivePolicy, MstDoubleTreeInitialRoute, SolverError,
        SolverInput, SolverSolution,
    },
};

use super::*;

fn problem(location_count: usize) -> OptimizationProblem {
    let locations: Vec<_> = (0..location_count)
        .map(|index| {
            serde_json::json!({
                "id": index.to_string(),
                "place_id": format!("place-{index}"),
                "open_time": "09:00",
                "close_time": "23:50",
                "stay_minutes": if index == 0 || index + 1 == location_count { 0 } else { 10 }
            })
        })
        .collect();
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "simulated-annealing-test",
        "locations": locations,
        "start_time": "09:00"
    }))
    .unwrap();
    request.try_into().unwrap()
}

fn directed_matrix(size: usize) -> TravelTimeMatrix {
    TravelTimeMatrix::new(
        (0..size)
            .map(|from| {
                (0..size)
                    .map(|to| {
                        if from == to {
                            0
                        } else if to == from + 1 {
                            10
                        } else if to > from {
                            30 + (to - from) as u32
                        } else {
                            100 + (from - to) as u32
                        }
                    })
                    .collect()
            })
            .collect(),
    )
    .unwrap()
}

fn deterministic_config(iterations: u64) -> SimulatedAnnealingConfig {
    SimulatedAnnealingConfig {
        initial_temperature: 1_000.0,
        cooling_rate: 0.99,
        minimum_temperature: 0.0001,
        iteration_limit: Some(iterations),
        time_limit_ms: None,
        seed: Some(42),
        penalties: InfeasiblePenaltyWeights::default(),
    }
}

include!("tests/neighborhood.rs");
include!("tests/feasibility.rs");
include!("tests/limits.rs");

#[test]
fn accepted_trajectory_samples_are_feasible_distinct_and_do_not_change_the_best_result() {
    let problem = problem(7);
    let matrix = directed_matrix(7);
    let cancellation = CancellationToken::new();
    let input = SolverInput {
        matrix: &matrix,
        problem: &problem,
        cancellation: &cancellation,
    };
    let solver = SimulatedAnnealingSolver::new(
        SimulatedAnnealingConfig {
            seed: Some(42),
            iteration_limit: Some(512),
            time_limit_ms: None,
            ..SimulatedAnnealingConfig::default()
        },
        FixedInitialRoute {
            solution: SolverSolution {
                visit_order: (0..7).collect(),
            },
        },
        MixedNeighborhoodStrategy,
        DefaultObjectivePolicy,
    )
    .unwrap();
    let plain = solver.solve_detailed(input).unwrap();
    let (traced, samples) = solver.solve_detailed_with_samples(input).unwrap();
    assert_eq!(plain.solution, traced.solution);
    assert_eq!(plain.metrics, traced.metrics);
    assert_eq!(plain.stats.iterations, traced.stats.iterations);
    assert_eq!(
        samples.len(),
        10,
        "a short SA run must retain accepted trajectory states, not only its winner"
    );
    let mut orders = std::collections::HashSet::new();
    for (solution, metrics) in samples {
        assert!(orders.insert(solution.visit_order.clone()));
        assert_ne!(solution, traced.initial_solution);
        let verified = evaluate_solution(&DefaultObjectivePolicy, input, &solution).unwrap();
        assert_eq!(verified, metrics);
        assert!(!crate::solver::ObjectiveEvaluator::compare(
            &DefaultObjectivePolicy,
            &traced.metrics,
            &metrics
        )
        .is_gt());
    }
}
