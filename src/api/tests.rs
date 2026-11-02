use std::time::Duration;

use super::*;
use crate::{
    domain::ScheduledStop,
    solver::{
        SolutionMetrics, SolverCandidate, SolverCandidateMetadata, SolverDiagnostics,
        SolverSolution,
    },
};
use serde_json::json;

fn request_with_locations(location_ids: &[&str]) -> OptimizeRouteRequest {
    serde_json::from_value(json!({
        "job_id": "route-api-test",
        "locations": location_ids
            .iter()
            .map(|id| json!({
                "id": id,
                "place_id": format!("place-{id}"),
                "open_time": "00:00",
                "close_time": "23:50",
                "stay_minutes": 0
            }))
            .collect::<Vec<_>>(),
        "start_time": "09:00"
    }))
    .unwrap()
}

include!("tests/request_validation.rs");
include!("tests/metadata.rs");
include!("tests/debug.rs");
