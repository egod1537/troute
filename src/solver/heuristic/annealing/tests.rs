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
