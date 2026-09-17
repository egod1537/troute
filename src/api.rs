use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    DomainError, Location, OptimizationProblem, RoutePlan, RoutingReference, TimeOfDay, TimeWindow,
};
use crate::events::{validate_job_id, JobIdError};

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
    pub debug: Option<DebugOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DebugOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_job_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocationInput {
    pub id: String,
    pub place_id: String,
    pub open_time: TimeOfDay,
    pub close_time: TimeOfDay,
    pub stay_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct OptimizeRouteResponse {
    pub route: Vec<RouteStopOutput>,
    pub total_travel_minutes: u32,
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
    pub(crate) fn from_plan(plan: RoutePlan, problem: &OptimizationProblem) -> Self {
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
    #[error("place_id must not be empty for location {location_id}")]
    EmptyPlaceId { location_id: String },
    #[error("place_id must not exceed {maximum} characters for location {location_id}")]
    PlaceIdTooLong { location_id: String, maximum: usize },
    #[error("duplicate location id: {0}")]
    DuplicateLocationId(String),
    #[error("invalid time window for location {location_id}: {source}")]
    InvalidTimeWindow {
        location_id: String,
        #[source]
        source: DomainError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn debug_duration_is_optional_bounded_and_round_trips() {
        let mut request = request_with_locations(&["A", "B"]);
        request.debug = Some(DebugOptions {
            min_job_duration_ms: Some(MAX_DEBUG_JOB_DURATION_MS),
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
        });
        assert!(matches!(
            OptimizationProblem::try_from(excessive),
            Err(RequestValidationError::DebugJobDurationTooLong {
                maximum: MAX_DEBUG_JOB_DURATION_MS,
                actual
            }) if actual == MAX_DEBUG_JOB_DURATION_MS + 1
        ));
    }
}
