use std::collections::HashSet;

use chrono::{LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    DomainError, Location, OptimizationProblem, RoutePlan, RoutingReference, StartPolicy,
    TimeOfDay, TimeWindow, VisitPolicy,
};
use crate::events::{validate_job_id, JobIdError};
use crate::routing::{
    normalize_country_code, RouteProviderName, RouteProviderSelection,
    RouteProviderSelectionSource, TravelMode,
};
use crate::solver::{
    PortfolioExperimentOptions, PortfolioTelemetrySummary, SolverCandidate,
    SolverCandidateMetadata, SolverDiagnostics, SolverOutcomeKind, SolverProofStatus,
    TIME_SLOT_MINUTES,
};

const MAX_LOCATIONS: usize = 500;
const MAX_STRING_CHARACTERS: usize = 512;
const INPUT_TIME_GRANULARITY_MINUTES: u32 = 10;
pub const MAX_DEBUG_JOB_DURATION_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptimizeRouteRequest {
    pub job_id: String,
    pub locations: Vec<LocationInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visit_policy: Option<VisitPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_policy: Option<StartPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<TimeOfDay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub travel_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "timeZone")]
    pub time_zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub travel_mode: Option<TravelMode>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "countryCode"
    )]
    pub country_code: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        alias = "provider",
        alias = "routeProvider"
    )]
    pub route_provider: Option<RouteProviderName>,
    /// Build a local straight-line travel-time estimate from location
    /// coordinates instead of querying a routing provider.
    #[serde(default, skip_serializing_if = "is_false")]
    pub use_euclidean_matrix: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub travel_time_matrix: Option<Vec<Vec<u32>>>,
    /// Sparse matrix form. JSON numbers are travel minutes and `null` marks a
    /// directed arc that is known to be unreachable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_travel_time_matrix: Option<Vec<Vec<Option<u32>>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub portfolio_experiment: Option<PortfolioExperimentOptions>,
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

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocationInput {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub place_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    pub open_time: TimeOfDay,
    pub close_time: TimeOfDay,
    pub stay_minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<u8>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OptimizeRouteResponse {
    pub route: Vec<RouteStopOutput>,
    pub total_travel_minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_policy: Option<StartPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_start_time: Option<TimeOfDay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partial: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visited_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_locations: Option<Vec<SkippedLocationOutput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_quality: Option<PartialResultQuality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver_candidates: Option<Vec<SolverCandidateOutput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver_diagnostics: Option<SolverDiagnosticsOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver_telemetry: Option<PortfolioTelemetrySummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_provider: Option<RouteProviderName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_selection_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_selection_source: Option<RouteProviderSelectionSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<TravelMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_departure_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrix_time_zone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_dependent_revalidation: Option<TimeDependentRevalidationOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TimeDependentRevalidationOutput {
    pub attempted: bool,
    pub applied: bool,
    pub revalidated_leg_count: usize,
    pub changed_leg_count: usize,
    pub max_delta_minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_feasible: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning_detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SkippedLocationOutput {
    pub location_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PartialResultQuality {
    Proven,
    BestFound,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SolverDiagnosticsOutput {
    pub total_budget_ms: u64,
    pub total_elapsed_ms: u64,
    pub baseline_elapsed_ms: u64,
    pub sa_elapsed_ms: u64,
    pub sa_run_count: u64,
    pub global_best_updates: u64,
    pub termination_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SolverCandidateOutput {
    pub strategy: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<u64>,
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
    /// Compatibility alias retained for existing clients. It contains the
    /// selected start for all start policies.
    pub latest_start: TimeOfDay,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<TimeOfDay>,
    pub finish_time: TimeOfDay,
    pub travel_minutes: u32,
    pub wait_minutes: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visited_optional_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visited_optional_priority: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct SolverCandidateMetadataOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_queued_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_started_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_finished_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reserved_slot_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_hint_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler_score: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<SolverOutcomeKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proof_status: Option<SolverProofStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objective_phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_bound: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_gap_ppm: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_process_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
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
    pub insert_optional_move_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_optional_move_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace_optional_move_count: Option<u64>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub improved_global_best: Option<bool>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_start_time: Option<TimeOfDay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub departure_time: Option<TimeOfDay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stay_minutes: Option<u32>,
}

impl TryFrom<OptimizeRouteRequest> for OptimizationProblem {
    type Error = RequestValidationError;

    fn try_from(request: OptimizeRouteRequest) -> Result<Self, Self::Error> {
        validate_job_id(&request.job_id).map_err(RequestValidationError::InvalidJobId)?;
        if let Some(country_code) = request.country_code.as_deref() {
            normalize_country_code(country_code)
                .map_err(RequestValidationError::InvalidCountryCode)?;
        }
        requested_routing_departure(
            &request,
            request
                .start_time
                .unwrap_or_else(|| TimeOfDay::from_minutes(0).expect("midnight is valid")),
        )?;
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
        let start_policy = request.start_policy.unwrap_or_default();
        let visit_policy = request.visit_policy.unwrap_or_default();
        let start_time = match (start_policy, request.start_time) {
            (StartPolicy::Fixed, None) => {
                return Err(RequestValidationError::FixedStartTimeRequired)
            }
            (_, Some(start_time)) => start_time,
            (_, None) => TimeOfDay::from_minutes(0).expect("midnight is a valid time"),
        };
        if request.travel_time_matrix.is_some() && request.sparse_travel_time_matrix.is_some() {
            return Err(RequestValidationError::MultipleTravelTimeMatrices);
        }
        let has_supplied_matrix =
            request.travel_time_matrix.is_some() || request.sparse_travel_time_matrix.is_some();
        if request.use_euclidean_matrix && has_supplied_matrix {
            return Err(RequestValidationError::EuclideanMatrixConflict);
        }
        if let Some(matrix) = request.travel_time_matrix.as_ref() {
            if matrix.len() != request.locations.len() {
                return Err(RequestValidationError::TravelTimeMatrixSizeMismatch {
                    expected: request.locations.len(),
                    actual: matrix.len(),
                });
            }
            for (row, values) in matrix.iter().enumerate() {
                if values.len() != request.locations.len() {
                    return Err(RequestValidationError::TravelTimeMatrixRowSizeMismatch {
                        row,
                        expected: request.locations.len(),
                        actual: values.len(),
                    });
                }
                if values[row] != 0 {
                    return Err(RequestValidationError::TravelTimeMatrixDiagonalNonzero {
                        index: row,
                        actual: values[row],
                    });
                }
            }
        }
        if let Some(matrix) = request.sparse_travel_time_matrix.as_ref() {
            if matrix.len() != request.locations.len() {
                return Err(RequestValidationError::TravelTimeMatrixSizeMismatch {
                    expected: request.locations.len(),
                    actual: matrix.len(),
                });
            }
            for (row, values) in matrix.iter().enumerate() {
                if values.len() != request.locations.len() {
                    return Err(RequestValidationError::TravelTimeMatrixRowSizeMismatch {
                        row,
                        expected: request.locations.len(),
                        actual: values.len(),
                    });
                }
                match values[row] {
                    Some(0) => {}
                    Some(actual) => {
                        return Err(RequestValidationError::TravelTimeMatrixDiagonalNonzero {
                            index: row,
                            actual,
                        })
                    }
                    None => {
                        return Err(
                            RequestValidationError::TravelTimeMatrixDiagonalUnreachable {
                                index: row,
                            },
                        )
                    }
                }
            }
        }
        let location_count = request.locations.len();
        let mut ids = HashSet::with_capacity(location_count);
        let mut locations = Vec::with_capacity(location_count);

        for (location_index, input) in request.locations.into_iter().enumerate() {
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
            if !has_supplied_matrix
                && !request.use_euclidean_matrix
                && input.place_id.trim().is_empty()
            {
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
            match (input.latitude, input.longitude) {
                (Some(latitude), Some(longitude)) => {
                    if !latitude.is_finite() || !(-90.0..=90.0).contains(&latitude) {
                        return Err(RequestValidationError::InvalidLatitude {
                            location_id: input.id,
                            actual: latitude,
                        });
                    }
                    if !longitude.is_finite() || !(-180.0..=180.0).contains(&longitude) {
                        return Err(RequestValidationError::InvalidLongitude {
                            location_id: input.id,
                            actual: longitude,
                        });
                    }
                }
                (None, None) if request.use_euclidean_matrix => {
                    return Err(RequestValidationError::CoordinatesRequired {
                        location_id: input.id,
                    });
                }
                (None, None) => {}
                _ => {
                    return Err(RequestValidationError::IncompleteCoordinates {
                        location_id: input.id,
                    });
                }
            }
            if !ids.insert(input.id.clone()) {
                return Err(RequestValidationError::DuplicateLocationId(input.id));
            }

            for (field, time) in [
                ("open_time", input.open_time),
                ("close_time", input.close_time),
            ] {
                if time.minutes() % INPUT_TIME_GRANULARITY_MINUTES as u16 != 0 {
                    return Err(RequestValidationError::TimeNotTenMinuteAligned {
                        location_id: input.id.clone(),
                        field,
                        actual: time,
                    });
                }
            }
            if input.stay_minutes % INPUT_TIME_GRANULARITY_MINUTES != 0 {
                return Err(RequestValidationError::StayMinutesNotTenMinuteMultiple {
                    location_id: input.id.clone(),
                    actual: input.stay_minutes,
                });
            }
            let endpoint = location_index == 0 || location_index + 1 == location_count;
            if input.required == Some(false)
                && (endpoint || visit_policy == VisitPolicy::AllRequired)
            {
                return Err(RequestValidationError::RequiredLocationCannotBeOptional {
                    location_id: input.id,
                });
            }
            let priority = input.priority.unwrap_or(1);
            if !(1..=100).contains(&priority) {
                return Err(RequestValidationError::InvalidLocationPriority {
                    location_id: input.id,
                    actual: priority,
                });
            }
            let required = visit_policy == VisitPolicy::AllRequired
                || endpoint
                || input.required.unwrap_or(false);

            let time_window =
                TimeWindow::new(input.open_time, input.close_time).map_err(|source| {
                    RequestValidationError::InvalidTimeWindow {
                        location_id: input.id.clone(),
                        source,
                    }
                })?;

            locations.push(
                Location::new(
                    input.id,
                    RoutingReference::GooglePlaceId(input.place_id),
                    time_window,
                    input.stay_minutes,
                )
                .with_visit_metadata(required, priority),
            );
        }

        Ok(OptimizationProblem::with_policies(
            locations,
            start_time,
            start_policy,
            visit_policy,
        ))
    }
}

impl OptimizeRouteResponse {
    /// Removes opt-in portfolio fields so ordinary `/optimize` traffic keeps
    /// its historical JSON shape. The unmodified response can still be stored
    /// for the versioned telemetry endpoints before this method is called.
    pub(crate) fn strip_portfolio_extensions(&mut self) {
        self.solver_telemetry = None;
        if let Some(candidates) = self.solver_candidates.as_mut() {
            for candidate in candidates {
                candidate.solver_id = None;
                candidate.run_id = None;
                if let Some(objective) = candidate.objective_score.as_mut() {
                    objective.visited_optional_count = None;
                    objective.visited_optional_priority = None;
                }
                candidate.metadata.queued_ms = None;
                candidate.metadata.job_queued_at_ms = None;
                candidate.metadata.job_started_at_ms = None;
                candidate.metadata.job_finished_at_ms = None;
                candidate.metadata.reserved_slot_ms = None;
                candidate.metadata.cpu_ms = None;
                candidate.metadata.peak_rss_bytes = None;
                candidate.metadata.memory_hint_bytes = None;
                candidate.metadata.scheduler_reason = None;
                candidate.metadata.scheduler_score = None;
                candidate.metadata.thread_count = None;
                candidate.metadata.improved_global_best = None;
            }
        }
    }

    pub(crate) fn from_plan(
        plan: RoutePlan,
        problem: &OptimizationProblem,
        diagnostics: Option<&SolverDiagnostics>,
    ) -> Self {
        let schedule_objective = ScheduleObjective::from_plan(&plan);
        let mut visited = vec![false; problem.locations().len()];
        for stop in &plan.stops {
            visited[stop.location_index] = true;
        }
        let partial_mode = problem.visit_policy() == VisitPolicy::BestEffortOnFailure;
        let skipped_locations = partial_mode.then(|| {
            problem
                .locations()
                .iter()
                .enumerate()
                .filter(|(index, location)| !location.required() && !visited[*index])
                .map(|(_, location)| SkippedLocationOutput {
                    location_id: location.id().to_owned(),
                    reason: "NOT_SELECTED_DUE_TO_CONSTRAINTS".to_owned(),
                })
                .collect::<Vec<_>>()
        });
        let partial = skipped_locations
            .as_ref()
            .map(|skipped| !skipped.is_empty());
        let result_quality = partial_mode.then(|| {
            if diagnostics
                .is_some_and(|diagnostics| diagnostics.selected_strategy == "exact_bit_dp")
            {
                PartialResultQuality::Proven
            } else {
                PartialResultQuality::BestFound
            }
        });
        let best_route: Vec<String> = plan
            .stops
            .iter()
            .map(|stop| problem.locations()[stop.location_index].id().to_owned())
            .collect();
        let route: Vec<RouteStopOutput> = plan
            .stops
            .into_iter()
            .enumerate()
            .map(|(order, stop)| RouteStopOutput {
                location_id: problem.locations()[stop.location_index].id().to_owned(),
                order,
                arrival_time: stop.arrival_time,
                service_start_time: Some(stop.service_start_time),
                departure_time: stop.departure_time,
                wait_minutes: Some(stop.wait_minutes),
                stay_minutes: Some(stop.stay_minutes),
            })
            .collect();

        let selected_start_time = route.first().map(|stop| stop.arrival_time);
        let visited_count = partial_mode.then_some(route.len());

        Self {
            route,
            total_travel_minutes: plan.total_travel_minutes,
            start_policy: Some(problem.start_policy()),
            selected_start_time,
            partial,
            visited_count,
            skipped_locations,
            result_quality,
            solver_candidates: diagnostics.map(|diagnostics| {
                let mut best_emitted = false;
                diagnostics
                    .candidates
                    .iter()
                    .map(|candidate| {
                        let best =
                            !best_emitted && candidate.strategy == diagnostics.selected_strategy;
                        best_emitted |= best;
                        SolverCandidateOutput::from_candidate(
                            candidate,
                            best,
                            problem,
                            best.then_some((&best_route, schedule_objective)),
                        )
                    })
                    .collect()
            }),
            solver_diagnostics: diagnostics.map(|diagnostics| SolverDiagnosticsOutput {
                total_budget_ms: diagnostics.total_budget_ms,
                total_elapsed_ms: diagnostics.total_elapsed_ms,
                baseline_elapsed_ms: diagnostics.baseline_elapsed_ms,
                sa_elapsed_ms: diagnostics.sa_elapsed_ms,
                sa_run_count: diagnostics.sa_run_count,
                global_best_updates: diagnostics.global_best_updates,
                termination_reason: diagnostics.termination_reason.clone(),
            }),
            solver_telemetry: diagnostics
                .and_then(|diagnostics| diagnostics.portfolio_telemetry.clone()),
            selected_provider: None,
            provider_selection_reason: None,
            provider_selection_source: None,
            country_code: None,
            mode: None,
            matrix_departure_time: None,
            matrix_time_zone: None,
            time_dependent_revalidation: None,
        }
    }

    pub(crate) fn set_provider_metadata(
        &mut self,
        selection: Option<&RouteProviderSelection>,
        country_code: Option<String>,
        mode: TravelMode,
    ) {
        if let Some(selection) = selection {
            self.selected_provider = Some(selection.provider);
            self.provider_selection_reason = Some(selection.reason.clone());
            self.provider_selection_source = Some(selection.source);
            self.country_code = country_code;
            self.mode = Some(mode);
        }
    }

    pub(crate) fn set_routing_time_metadata(
        &mut self,
        departure_time: chrono::DateTime<Utc>,
        time_zone: Option<String>,
    ) {
        self.matrix_departure_time = Some(departure_time.to_rfc3339());
        self.matrix_time_zone = time_zone;
    }

    pub(crate) fn set_time_dependent_revalidation(
        &mut self,
        value: TimeDependentRevalidationOutput,
    ) {
        self.time_dependent_revalidation = Some(value);
    }
}

pub(crate) fn requested_routing_departure(
    request: &OptimizeRouteRequest,
    start_time: TimeOfDay,
) -> Result<Option<(chrono::DateTime<Utc>, String)>, RequestValidationError> {
    let (date, time_zone) = match (&request.travel_date, &request.time_zone) {
        (None, None) => return Ok(None),
        (Some(_), None) => return Err(RequestValidationError::TravelDateRequiresTimeZone),
        (None, Some(_)) => return Err(RequestValidationError::TimeZoneRequiresTravelDate),
        (Some(date), Some(time_zone)) => (date, time_zone),
    };
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| {
        RequestValidationError::InvalidTravelDate(request.travel_date.clone().unwrap_or_default())
    })?;
    let zone = time_zone
        .parse::<chrono_tz::Tz>()
        .map_err(|_| RequestValidationError::InvalidTimeZone(time_zone.clone()))?;
    if let Some(country) = request.country_code.as_deref() {
        let expected = match country.to_ascii_uppercase().as_str() {
            "KR" => Some("Asia/Seoul"),
            "JP" => Some("Asia/Tokyo"),
            _ => None,
        };
        if expected.is_some_and(|expected| expected != time_zone) {
            return Err(RequestValidationError::CountryTimeZoneMismatch {
                country_code: country.to_ascii_uppercase(),
                time_zone: time_zone.clone(),
            });
        }
    }
    let minute = start_time.minutes();
    let time = NaiveTime::from_hms_opt(u32::from(minute / 60), u32::from(minute % 60), 0)
        .expect("validated TimeOfDay always forms a NaiveTime");
    let local = NaiveDateTime::new(date, time);
    let zoned = match zone.from_local_datetime(&local) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(_, _) => {
            return Err(RequestValidationError::AmbiguousLocalDeparture {
                local: local.to_string(),
                time_zone: time_zone.clone(),
            })
        }
        LocalResult::None => {
            return Err(RequestValidationError::NonexistentLocalDeparture {
                local: local.to_string(),
                time_zone: time_zone.clone(),
            })
        }
    };
    Ok(Some((zoned.with_timezone(&Utc), time_zone.clone())))
}

impl SolverCandidateOutput {
    fn from_candidate(
        candidate: &SolverCandidate,
        best: bool,
        problem: &OptimizationProblem,
        selected_schedule: Option<(&[String], Option<ScheduleObjective>)>,
    ) -> Self {
        let route = selected_schedule
            .map(|(route, _)| route.to_vec())
            .or_else(|| {
                candidate.route.as_ref().map(|solution| {
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
            })
            .unwrap_or_default();
        let objective_score = selected_schedule
            .and_then(|(_, objective)| objective)
            .map(ObjectiveScoreOutput::from_schedule)
            .or_else(|| {
                candidate.objective_score.and_then(|score| {
                    Some(ObjectiveScoreOutput {
                        latest_start: TimeOfDay::from_minutes(
                            score.start_time_slot * TIME_SLOT_MINUTES as u16,
                        )
                        .ok()?,
                        start_time: Some(
                            TimeOfDay::from_minutes(
                                score.start_time_slot * TIME_SLOT_MINUTES as u16,
                            )
                            .ok()?,
                        ),
                        finish_time: TimeOfDay::from_minutes(
                            score.finish_time_slot * TIME_SLOT_MINUTES as u16,
                        )
                        .ok()?,
                        travel_minutes: score.travel_minutes,
                        wait_minutes: score.wait_minutes,
                        visited_optional_count: Some(score.visited_optional_count),
                        visited_optional_priority: Some(score.visited_optional_priority),
                    })
                })
            });
        Self {
            strategy: candidate.strategy.clone(),
            solver_id: candidate.metadata.solver_id.clone().or_else(|| {
                Some(
                    if candidate.strategy.starts_with("sa_") {
                        "sa"
                    } else {
                        &candidate.strategy
                    }
                    .to_owned(),
                )
            }),
            run_id: candidate.metadata.run_id,
            best,
            route,
            feasible: candidate.feasible,
            objective_score,
            elapsed_ms: u64::try_from(candidate.elapsed.as_millis()).unwrap_or(u64::MAX),
            metadata: SolverCandidateMetadataOutput::from(&candidate.metadata),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ScheduleObjective {
    start_time: TimeOfDay,
    finish_time: TimeOfDay,
    travel_minutes: u32,
    wait_minutes: u32,
}

impl ScheduleObjective {
    fn from_plan(plan: &RoutePlan) -> Option<Self> {
        Some(Self {
            start_time: plan.stops.first()?.arrival_time,
            finish_time: plan.stops.last()?.departure_time?,
            travel_minutes: plan.total_travel_minutes,
            wait_minutes: plan.stops.iter().map(|stop| stop.wait_minutes).sum(),
        })
    }
}

impl ObjectiveScoreOutput {
    fn from_schedule(objective: ScheduleObjective) -> Self {
        Self {
            latest_start: objective.start_time,
            start_time: Some(objective.start_time),
            finish_time: objective.finish_time,
            travel_minutes: objective.travel_minutes,
            wait_minutes: objective.wait_minutes,
            visited_optional_count: None,
            visited_optional_priority: None,
        }
    }
}

impl From<&SolverCandidateMetadata> for SolverCandidateMetadataOutput {
    fn from(metadata: &SolverCandidateMetadata) -> Self {
        Self {
            job_queued_at_ms: metadata.job_queued_at_ms,
            job_started_at_ms: metadata.job_started_at_ms,
            job_finished_at_ms: metadata.job_finished_at_ms,
            queued_ms: metadata.queued_ms,
            reserved_slot_ms: metadata.reserved_slot_ms,
            cpu_ms: metadata.cpu_ms,
            peak_rss_bytes: metadata.peak_rss_bytes,
            memory_hint_bytes: metadata.memory_hint_bytes,
            scheduler_reason: metadata.scheduler_reason.clone(),
            scheduler_score: metadata.scheduler_score.clone(),
            outcome: metadata.outcome,
            proof_status: metadata.proof_status,
            objective_phase: metadata.objective_phase.clone(),
            best_bound: metadata.best_bound,
            relative_gap_ppm: metadata.relative_gap_ppm,
            thread_count: metadata.thread_count,
            source_version: metadata.source_version.clone(),
            native_status: metadata.native_status.clone(),
            child_process_id: metadata.child_process_id,
            exit_code: metadata.exit_code,
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
            insert_optional_move_count: metadata.insert_optional_move_count,
            remove_optional_move_count: metadata.remove_optional_move_count,
            replace_optional_move_count: metadata.replace_optional_move_count,
            accepted_worse_moves: metadata.accepted_worse_moves,
            infeasible_candidates: metadata.infeasible_candidates,
            accepted_infeasible_moves: metadata.accepted_infeasible_moves,
            best_feasible: metadata.best_feasible,
            iteration_count: metadata.iteration_count,
            accepted_moves: metadata.accepted_moves,
            improved_moves: metadata.improved_moves,
            seed: metadata.seed,
            improved_global_best: metadata.improved_global_best,
            timed_out: metadata.timed_out,
            error: metadata.error.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum RequestValidationError {
    #[error(transparent)]
    InvalidJobId(JobIdError),
    #[error("invalid country_code: {0}")]
    InvalidCountryCode(String),
    #[error("travel_date requires time_zone")]
    TravelDateRequiresTimeZone,
    #[error("time_zone requires travel_date")]
    TimeZoneRequiresTravelDate,
    #[error("travel_date must use YYYY-MM-DD and be a real calendar date: {0}")]
    InvalidTravelDate(String),
    #[error("time_zone must be a valid IANA identifier: {0}")]
    InvalidTimeZone(String),
    #[error("country_code {country_code} does not match time_zone {time_zone}")]
    CountryTimeZoneMismatch {
        country_code: String,
        time_zone: String,
    },
    #[error("local departure {local} is ambiguous in {time_zone}")]
    AmbiguousLocalDeparture { local: String, time_zone: String },
    #[error("local departure {local} does not exist in {time_zone}")]
    NonexistentLocalDeparture { local: String, time_zone: String },
    #[error("start_time is required when start_policy is FIXED")]
    FixedStartTimeRequired,
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
    #[error("{field} must use a 10-minute boundary for location {location_id} (actual {actual})")]
    TimeNotTenMinuteAligned {
        location_id: String,
        field: &'static str,
        actual: TimeOfDay,
    },
    #[error(
        "stay_minutes must be a non-negative multiple of 10 for location {location_id} (actual {actual})"
    )]
    StayMinutesNotTenMinuteMultiple { location_id: String, actual: u32 },
    #[error("start/end and ALL_REQUIRED locations cannot set required=false ({location_id})")]
    RequiredLocationCannotBeOptional { location_id: String },
    #[error("priority must be between 1 and 100 for location {location_id} (actual {actual})")]
    InvalidLocationPriority { location_id: String, actual: u8 },
    #[error("provide only one of travel_time_matrix and sparse_travel_time_matrix")]
    MultipleTravelTimeMatrices,
    #[error(
        "use_euclidean_matrix cannot be combined with travel_time_matrix or sparse_travel_time_matrix"
    )]
    EuclideanMatrixConflict,
    #[error("latitude and longitude are required for location {location_id} when use_euclidean_matrix is true")]
    CoordinatesRequired { location_id: String },
    #[error("latitude and longitude must be supplied together for location {location_id}")]
    IncompleteCoordinates { location_id: String },
    #[error("latitude must be finite and between -90 and 90 for location {location_id} (actual {actual})")]
    InvalidLatitude { location_id: String, actual: f64 },
    #[error("longitude must be finite and between -180 and 180 for location {location_id} (actual {actual})")]
    InvalidLongitude { location_id: String, actual: f64 },
    #[error(
        "travel_time_matrix has {actual} rows; expected {expected} for the supplied locations"
    )]
    TravelTimeMatrixSizeMismatch { expected: usize, actual: usize },
    #[error("travel_time_matrix row {row} has length {actual}; expected {expected}")]
    TravelTimeMatrixRowSizeMismatch {
        row: usize,
        expected: usize,
        actual: usize,
    },
    #[error("travel_time_matrix diagonal entry [{index}][{index}] must be 0; actual {actual}")]
    TravelTimeMatrixDiagonalNonzero { index: usize, actual: u32 },
    #[error("sparse_travel_time_matrix diagonal entry [{index}][{index}] must be 0, not null")]
    TravelTimeMatrixDiagonalUnreachable { index: usize },
    #[error("invalid time window for location {location_id}: {source}")]
    InvalidTimeWindow {
        location_id: String,
        #[source]
        source: DomainError,
    },
}

#[cfg(test)]
mod tests;
