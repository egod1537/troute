use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    DomainError, Location, OptimizationProblem, RoutePlan, RoutingReference, TimeOfDay, TimeWindow,
};
use crate::events::{validate_job_id, JobIdError};
use crate::solver::{
    SolverCandidate, SolverCandidateMetadata, SolverDiagnostics, TIME_SLOT_MINUTES,
};

const MAX_LOCATIONS: usize = 500;
const MAX_STRING_CHARACTERS: usize = 512;
pub const MAX_DEBUG_JOB_DURATION_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizeRouteRequest {
    pub job_id: String,
    pub locations: Vec<LocationInput>,
    pub start_time: TimeOfDay,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub travel_time_matrix: Option<Vec<Vec<u32>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DebugOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_job_duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shuffle_result_route: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shuffle_seed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocationInput {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub place_id: String,
    pub open_time: TimeOfDay,
    pub close_time: TimeOfDay,
    pub stay_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OptimizeRouteResponse {
    pub route: Vec<RouteStopOutput>,
    pub total_travel_minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver_candidates: Option<Vec<SolverCandidateOutput>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SolverCandidateOutput {
    pub strategy: String,
    pub best: bool,
    pub route: Vec<String>,
    pub feasible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective_score: Option<ObjectiveScoreOutput>,
    pub elapsed_ms: u64,
    pub metadata: SolverCandidateMetadataOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct ObjectiveScoreOutput {
    pub latest_start: TimeOfDay,
    pub finish_time: TimeOfDay,
    pub travel_minutes: u32,
    pub wait_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SolverCandidateMetadataOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontier_state_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontier_cell_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_sizes: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_order_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_order: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_details: Option<Vec<ClusterDiagnosticOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_before_improvement: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_after_improvement: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub improvement_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relocate_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub two_opt_enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symmetric_distance_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mst_cost: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mst_edge_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mst_edges: Option<Vec<MstEdgeDiagnosticOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub euler_tour: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shortcut_route: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub odd_vertices: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub odd_vertex_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matching_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matching_cost: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matching_pairs: Option<Vec<MatchingPairDiagnosticOutput>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_strategy: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_route: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_route: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_score: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_score: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_temperature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_temperature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooling_rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_move_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relocate_move_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub two_opt_move_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_worse_moves: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub infeasible_candidates: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_infeasible_moves: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_feasible: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iteration_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted_moves: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub improved_moves: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    pub timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClusterDiagnosticOutput {
    pub cluster: usize,
    pub members: Vec<String>,
    pub route: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit: Option<String>,
    pub state_count: usize,
    pub frontier_state_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MstEdgeDiagnosticOutput {
    pub from: String,
    pub to: String,
    pub distance: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct MatchingPairDiagnosticOutput {
    pub left: String,
    pub right: String,
    pub distance: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct RouteStopOutput {
    pub location_id: String,
    pub order: usize,
    pub arrival_time: TimeOfDay,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub departure_time: Option<TimeOfDay>,
}

impl TryFrom<OptimizeRouteRequest> for OptimizationProblem {
    type Error = RequestValidationError;

    fn try_from(request: OptimizeRouteRequest) -> Result<Self, Self::Error> {
        validate_job_id(&request.job_id).map_err(RequestValidationError::InvalidJobId)?;
        if let Some(actual) = request
            .debug
            .as_ref()
            .and_then(|debug| debug.min_job_duration_ms)
            .filter(|duration| *duration > MAX_DEBUG_JOB_DURATION_MS)
        {
            return Err(RequestValidationError::DebugJobDurationTooLong {
                maximum: MAX_DEBUG_JOB_DURATION_MS,
                actual,
            });
        }
        if request.locations.len() < 2 {
            return Err(RequestValidationError::TooFewLocations {
                minimum: 2,
                actual: request.locations.len(),
            });
        }
        if request.locations.len() > MAX_LOCATIONS {
            return Err(RequestValidationError::TooManyLocations {
                maximum: MAX_LOCATIONS,
                actual: request.locations.len(),
            });
        }
        if request.travel_time_matrix.is_some() {
            return Err(RequestValidationError::CallerSuppliedMatrixUnsupported);
        }
        let mut ids = HashSet::with_capacity(request.locations.len());
        let mut locations = Vec::with_capacity(request.locations.len());

        for input in request.locations {
            if input.id.trim().is_empty() {
                return Err(RequestValidationError::EmptyLocationId);
            }
            if input.id.chars().count() > MAX_STRING_CHARACTERS {
                return Err(RequestValidationError::LocationIdTooLong {
                    maximum: MAX_STRING_CHARACTERS,
                });
            }
            if input
                .name
                .as_ref()
                .is_some_and(|name| name.chars().count() > MAX_STRING_CHARACTERS)
            {
                return Err(RequestValidationError::LocationNameTooLong {
                    location_id: input.id,
                    maximum: MAX_STRING_CHARACTERS,
                });
            }
            if input.place_id.trim().is_empty() {
                return Err(RequestValidationError::EmptyPlaceId {
                    location_id: input.id,
                });
            }
            if input.place_id.chars().count() > MAX_STRING_CHARACTERS {
                return Err(RequestValidationError::PlaceIdTooLong {
                    location_id: input.id,
                    maximum: MAX_STRING_CHARACTERS,
                });
            }
            if !ids.insert(input.id.clone()) {
                return Err(RequestValidationError::DuplicateLocationId(input.id));
            }

            let time_window =
                TimeWindow::new(input.open_time, input.close_time).map_err(|source| {
                    RequestValidationError::InvalidTimeWindow {
                        location_id: input.id.clone(),
                        source,
                    }
                })?;

            locations.push(Location::new(
                input.id,
                RoutingReference::GooglePlaceId(input.place_id),
                time_window,
                input.stay_minutes,
            ));
        }

        Ok(OptimizationProblem::new(locations, request.start_time))
    }
}

impl OptimizeRouteResponse {
    pub(crate) fn from_plan(
        plan: RoutePlan,
        problem: &OptimizationProblem,
        diagnostics: Option<&SolverDiagnostics>,
    ) -> Self {
        let route = plan
            .stops
            .into_iter()
            .enumerate()
            .map(|(order, stop)| RouteStopOutput {
                location_id: problem.locations()[stop.location_index].id().to_owned(),
                order,
                arrival_time: stop.arrival_time,
                departure_time: stop.departure_time,
            })
            .collect();

        Self {
            route,
            total_travel_minutes: plan.total_travel_minutes,
            solver_candidates: diagnostics.map(|diagnostics| {
                diagnostics
                    .candidates
                    .iter()
                    .map(|candidate| {
                        SolverCandidateOutput::from_candidate(
                            candidate,
                            candidate.strategy == diagnostics.selected_strategy,
                            problem,
                        )
                    })
                    .collect()
            }),
        }
    }
}

impl SolverCandidateOutput {
    fn from_candidate(
        candidate: &SolverCandidate,
        best: bool,
        problem: &OptimizationProblem,
    ) -> Self {
        let route = candidate
            .route
            .as_ref()
            .map(|solution| {
                solution
                    .visit_order
                    .iter()
                    .map(|&index| {
                        problem
                            .locations()
                            .get(index)
                            .map(|location| location.id().to_owned())
                            .unwrap_or_else(|| format!("#{index}"))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let objective_score = candidate.objective_score.and_then(|score| {
            Some(ObjectiveScoreOutput {
                latest_start: TimeOfDay::from_minutes(
                    score.start_time_slot * TIME_SLOT_MINUTES as u16,
                )
                .ok()?,
                finish_time: TimeOfDay::from_minutes(
                    score.finish_time_slot * TIME_SLOT_MINUTES as u16,
                )
                .ok()?,
                travel_minutes: score.travel_minutes,
                wait_minutes: score.wait_minutes,
            })
        });
        Self {
            strategy: candidate.strategy.clone(),
            best,
            route,
            feasible: candidate.feasible,
            objective_score,
            elapsed_ms: u64::try_from(candidate.elapsed.as_millis()).unwrap_or(u64::MAX),
            metadata: SolverCandidateMetadataOutput::from(&candidate.metadata),
        }
    }
}

impl From<&SolverCandidateMetadata> for SolverCandidateMetadataOutput {
    fn from(metadata: &SolverCandidateMetadata) -> Self {
        Self {
            state_count: metadata.state_count,
            frontier_state_count: metadata.frontier_state_count,
            frontier_cell_count: metadata.frontier_cell_count,
            cluster_count: metadata.cluster_count,
            cluster_sizes: metadata.cluster_sizes.clone(),
            cluster_strategy: metadata.cluster_strategy.clone(),
            cluster_order_strategy: metadata.cluster_order_strategy.clone(),
            cluster_order: metadata.cluster_order.clone(),
            cluster_details: metadata.cluster_details.as_ref().map(|clusters| {
                clusters
                    .iter()
                    .map(|cluster| ClusterDiagnosticOutput {
                        cluster: cluster.cluster,
                        members: cluster.members.clone(),
                        route: cluster.route.clone(),
                        entry: cluster.entry.clone(),
                        exit: cluster.exit.clone(),
                        state_count: cluster.state_count,
                        frontier_state_count: cluster.frontier_state_count,
                    })
                    .collect()
            }),
            score_before_improvement: metadata.score_before_improvement,
            score_after_improvement: metadata.score_after_improvement,
            improvement_strategy: metadata.improvement_strategy.clone(),
            swap_enabled: metadata.swap_enabled,
            relocate_enabled: metadata.relocate_enabled,
            two_opt_enabled: metadata.two_opt_enabled,
            symmetric_distance_strategy: metadata.symmetric_distance_strategy.clone(),
            mst_cost: metadata.mst_cost,
            mst_edge_count: metadata.mst_edge_count,
            mst_edges: metadata.mst_edges.as_ref().map(|edges| {
                edges
                    .iter()
                    .map(|edge| MstEdgeDiagnosticOutput {
                        from: edge.from.clone(),
                        to: edge.to.clone(),
                        distance: edge.distance,
                    })
                    .collect()
            }),
            euler_tour: metadata.euler_tour.clone(),
            shortcut_route: metadata.shortcut_route.clone(),
            odd_vertices: metadata.odd_vertices.clone(),
            odd_vertex_count: metadata.odd_vertex_count,
            matching_strategy: metadata.matching_strategy.clone(),
            matching_cost: metadata.matching_cost,
            matching_pairs: metadata.matching_pairs.as_ref().map(|pairs| {
                pairs
                    .iter()
                    .map(|pair| MatchingPairDiagnosticOutput {
                        left: pair.left.clone(),
                        right: pair.right.clone(),
                        distance: pair.distance,
                    })
                    .collect()
            }),
            initial_strategy: metadata.initial_strategy.clone(),
            initial_route: metadata.initial_route.clone(),
            final_route: metadata.final_route.clone(),
            initial_score: metadata.initial_score,
            final_score: metadata.final_score,
            initial_temperature: metadata.initial_temperature.clone(),
            final_temperature: metadata.final_temperature.clone(),
            cooling_rate: metadata.cooling_rate.clone(),
            swap_move_count: metadata.swap_move_count,
            relocate_move_count: metadata.relocate_move_count,
            two_opt_move_count: metadata.two_opt_move_count,
            accepted_worse_moves: metadata.accepted_worse_moves,
            infeasible_candidates: metadata.infeasible_candidates,
            accepted_infeasible_moves: metadata.accepted_infeasible_moves,
            best_feasible: metadata.best_feasible,
            iteration_count: metadata.iteration_count,
            accepted_moves: metadata.accepted_moves,
            improved_moves: metadata.improved_moves,
            seed: metadata.seed,
            timed_out: metadata.timed_out,
            error: metadata.error.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum RequestValidationError {
    #[error(transparent)]
    InvalidJobId(JobIdError),
    #[error("debug.min_job_duration_ms must not exceed {maximum} milliseconds (actual {actual})")]
    DebugJobDurationTooLong { maximum: u64, actual: u64 },
    #[error(
        "locations must contain at least start and end locations (minimum {minimum}, actual {actual})"
    )]
    TooFewLocations { minimum: usize, actual: usize },
    #[error("locations contains {actual} items; at most {maximum} are allowed")]
    TooManyLocations { maximum: usize, actual: usize },
    #[error("location id must not be empty")]
    EmptyLocationId,
    #[error("location id must not exceed {maximum} characters")]
    LocationIdTooLong { maximum: usize },
    #[error("location name must not exceed {maximum} characters for location {location_id}")]
    LocationNameTooLong { location_id: String, maximum: usize },
    #[error("place_id must not be empty for location {location_id}")]
    EmptyPlaceId { location_id: String },
    #[error("place_id must not exceed {maximum} characters for location {location_id}")]
    PlaceIdTooLong { location_id: String, maximum: usize },
    #[error("duplicate location id: {0}")]
    DuplicateLocationId(String),
    #[error("travel_time_matrix cannot be supplied; troute always obtains it from tcache")]
    CallerSuppliedMatrixUnsupported,
    #[error("invalid time window for location {location_id}: {source}")]
    InvalidTimeWindow {
        location_id: String,
        #[source]
        source: DomainError,
    },
}

#[cfg(test)]
mod tests {
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
                    "close_time": "23:59",
                    "stay_minutes": 0
                }))
                .collect::<Vec<_>>(),
            "start_time": "09:00"
        }))
        .unwrap()
    }

    #[test]
    fn zero_and_one_location_are_rejected() {
        for ids in [&[][..], &["A"][..]] {
            let error = OptimizationProblem::try_from(request_with_locations(ids)).unwrap_err();
            assert!(matches!(
                error,
                RequestValidationError::TooFewLocations {
                    minimum: 2,
                    actual
                } if actual == ids.len()
            ));
        }
    }

    #[test]
    fn two_locations_define_start_and_end() {
        let problem = OptimizationProblem::try_from(request_with_locations(&["A", "B"])).unwrap();

        assert_eq!(problem.start_location().id(), "A");
        assert_eq!(problem.end_location().id(), "B");
        assert!(problem.intermediate_locations().is_empty());
    }

    #[test]
    fn location_ids_remain_unique() {
        let error = OptimizationProblem::try_from(request_with_locations(&["A", "A"])).unwrap_err();
        assert!(matches!(
            error,
            RequestValidationError::DuplicateLocationId(id) if id == "A"
        ));
    }

    #[test]
    fn response_exposes_ranked_solver_candidates_and_best_marker() {
        let problem = OptimizationProblem::try_from(request_with_locations(&["A", "B"])).unwrap();
        let diagnostics = SolverDiagnostics {
            selected_strategy: "exact_bit_dp".to_owned(),
            candidates: vec![SolverCandidate {
                strategy: "exact_bit_dp".to_owned(),
                route: Some(SolverSolution {
                    visit_order: vec![0, 1],
                }),
                feasible: true,
                objective_score: Some(SolutionMetrics {
                    start_time_slot: 60,
                    finish_time_slot: 61,
                    travel_minutes: 10,
                    wait_minutes: 0,
                    score: 10,
                }),
                elapsed: Duration::from_millis(7),
                metadata: SolverCandidateMetadata {
                    state_count: Some(2),
                    frontier_state_count: Some(2),
                    frontier_cell_count: Some(2),
                    ..SolverCandidateMetadata::default()
                },
            }],
        };
        let response = OptimizeRouteResponse::from_plan(
            RoutePlan {
                stops: vec![
                    ScheduledStop {
                        location_index: 0,
                        arrival_time: TimeOfDay::from_minutes(600).unwrap(),
                        departure_time: Some(TimeOfDay::from_minutes(600).unwrap()),
                    },
                    ScheduledStop {
                        location_index: 1,
                        arrival_time: TimeOfDay::from_minutes(610).unwrap(),
                        departure_time: None,
                    },
                ],
                total_travel_minutes: 10,
            },
            &problem,
            Some(&diagnostics),
        );

        let candidate = &response.solver_candidates.unwrap()[0];
        assert!(candidate.best);
        assert!(candidate.feasible);
        assert_eq!(candidate.route, ["A", "B"]);
        assert_eq!(candidate.elapsed_ms, 7);
        assert_eq!(
            candidate.objective_score.unwrap().latest_start.to_string(),
            "10:00"
        );
        assert_eq!(candidate.metadata.state_count, Some(2));
        assert_eq!(candidate.metadata.frontier_state_count, Some(2));
        assert_eq!(candidate.metadata.frontier_cell_count, Some(2));
    }

    #[test]
    fn debug_duration_is_optional_bounded_and_round_trips() {
        let mut request = request_with_locations(&["A", "B"]);
        request.debug = Some(DebugOptions {
            min_job_duration_ms: Some(MAX_DEBUG_JOB_DURATION_MS),
            shuffle_result_route: None,
            shuffle_seed: None,
        });
        let serialized = serde_json::to_value(&request).unwrap();
        assert_eq!(
            serialized["debug"]["min_job_duration_ms"],
            MAX_DEBUG_JOB_DURATION_MS
        );
        OptimizationProblem::try_from(request).unwrap();

        let mut excessive = request_with_locations(&["A", "B"]);
        excessive.debug = Some(DebugOptions {
            min_job_duration_ms: Some(MAX_DEBUG_JOB_DURATION_MS + 1),
            shuffle_result_route: None,
            shuffle_seed: None,
        });
        assert!(matches!(
            OptimizationProblem::try_from(excessive),
            Err(RequestValidationError::DebugJobDurationTooLong {
                maximum: MAX_DEBUG_JOB_DURATION_MS,
                actual
            }) if actual == MAX_DEBUG_JOB_DURATION_MS + 1
        ));
    }

    #[test]
    fn debug_shuffle_options_are_optional_strict_and_round_trip() {
        let mut request = request_with_locations(&["A", "B", "C"]);
        request.debug = Some(DebugOptions {
            min_job_duration_ms: None,
            shuffle_result_route: Some(true),
            shuffle_seed: Some(1_234),
        });

        let serialized = serde_json::to_value(&request).unwrap();
        assert_eq!(serialized["debug"]["shuffle_result_route"], true);
        assert_eq!(serialized["debug"]["shuffle_seed"], 1_234);
        OptimizationProblem::try_from(request).unwrap();

        let invalid = serde_json::from_value::<OptimizeRouteRequest>(json!({
            "job_id": "route-api-test",
            "locations": [
                {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:59","stay_minutes":0},
                {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:59","stay_minutes":0}
            ],
            "start_time": "09:00",
            "debug": {"shuffle_result_route": "true"}
        }));
        assert!(invalid.is_err());
    }
}
