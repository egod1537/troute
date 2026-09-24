pub mod christofides;
mod clustered;
pub mod greedy;
pub mod mst;
pub mod random;
pub mod traits;

pub use christofides::*;
pub use clustered::*;
pub use greedy::*;
pub use mst::*;
pub use random::*;
pub use traits::*;

#[cfg(test)]
mod test_support {
    use crate::{
        api::OptimizeRouteRequest, cancellation::CancellationToken, domain::OptimizationProblem,
        matrix::TravelTimeMatrix, solver::SolverInput,
    };

    pub(super) fn problem(location_count: usize) -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "initial-route-test",
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

    pub(super) fn input<'a>(
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
}
