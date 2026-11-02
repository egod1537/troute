use std::collections::BTreeSet;

use rand::{rngs::StdRng, SeedableRng};

use crate::{
    api::OptimizeRouteRequest, cancellation::CancellationToken, domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
};

use super::*;
use crate::solver::{
    evaluate_solution, AverageSymmetricDistance, DefaultObjectivePolicy, ExactBitDpSolver,
    InitialRouteGenerator, MixedNeighborhoodStrategy, MstEdge, SimulatedAnnealingConfig,
    SimulatedAnnealingSolver, SolverError, SolverInput, SymmetricDistanceStrategy,
};

fn problem(location_count: usize) -> OptimizationProblem {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "christofides-test",
        "locations": (0..location_count).map(|index| serde_json::json!({
            "id": index.to_string(),
            "place_id": format!("p-{index}"),
            "open_time": "00:00",
            "close_time": "23:50",
            "stay_minutes": 0
        })).collect::<Vec<_>>(),
        "start_time": "00:00"
    }))
    .unwrap();
    request.try_into().unwrap()
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

fn line_metric(size: usize) -> TravelTimeMatrix {
    TravelTimeMatrix::new(
        (0..size)
            .map(|left| {
                (0..size)
                    .map(|right| left.abs_diff(right) as u32 * 10)
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

include!("tests/matching.rs");
include!("tests/generation.rs");

fn matching_cost(matching: &[PerfectMatchingEdge]) -> u64 {
    matching.iter().map(|edge| edge.distance).sum()
}
