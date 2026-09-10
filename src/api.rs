use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::{
    DomainError, Location, OptimizationProblem, RoutePlan, RoutingReference, TimeOfDay, TimeWindow,
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OptimizeRouteRequest {
    pub locations: Vec<LocationInput>,
    pub start_location_id: String,
    pub start_time: TimeOfDay,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct LocationInput {
    pub id: String,
    pub place_id: String,
    pub open_time: TimeOfDay,
    pub close_time: TimeOfDay,
    pub stay_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptimizeRouteResponse {
    pub route: Vec<RouteStopOutput>,
    pub total_travel_minutes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
        if request.locations.is_empty() {
            return Err(RequestValidationError::NoLocations);
        }

        let mut ids = HashSet::with_capacity(request.locations.len());
        let mut locations = Vec::with_capacity(request.locations.len());

        for input in request.locations {
            if input.id.trim().is_empty() {
                return Err(RequestValidationError::EmptyLocationId);
            }
            if input.place_id.trim().is_empty() {
                return Err(RequestValidationError::EmptyPlaceId {
                    location_id: input.id,
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

        let start_index = locations
            .iter()
            .position(|location| location.id() == request.start_location_id)
            .ok_or(RequestValidationError::UnknownStartLocation(
                request.start_location_id,
            ))?;

        Ok(OptimizationProblem::new(
            locations,
            start_index,
            request.start_time,
        ))
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
    #[error("at least one location is required")]
    NoLocations,
    #[error("location id must not be empty")]
    EmptyLocationId,
    #[error("place_id must not be empty for location {location_id}")]
    EmptyPlaceId { location_id: String },
    #[error("duplicate location id: {0}")]
    DuplicateLocationId(String),
    #[error("start_location_id does not exist in locations: {0}")]
    UnknownStartLocation(String),
    #[error("invalid time window for location {location_id}: {source}")]
    InvalidTimeWindow {
        location_id: String,
        #[source]
        source: DomainError,
    },
}
