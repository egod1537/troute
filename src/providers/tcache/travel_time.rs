use serde::{Deserialize, Serialize};

use crate::{
    domain::{Location, RoutingReference},
    routing::{RoutingError, TravelTime},
};

use super::client::pair_context;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RouteRequest<'a> {
    pub(super) locations: Vec<RouteLocation<'a>>,
    pub(super) mode: &'static str,
    pub(super) departure_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) provider: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) language_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) region_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) routing_preference: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) units: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RouteLocation<'a> {
    pub(super) place_id: &'a str,
}

impl<'a> From<&'a Location> for RouteLocation<'a> {
    fn from(location: &'a Location) -> Self {
        let RoutingReference::GooglePlaceId(place_id) = location.routing_reference();
        Self { place_id }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateJobResponse {
    pub(super) job_id: String,
}

#[derive(Deserialize)]
pub(super) struct JobStatusResponse {
    pub(super) status: String,
    pub(super) error: Option<TcacheErrorBody>,
}

#[derive(Deserialize)]
pub(super) struct TcacheErrorBody {
    pub(super) code: String,
    pub(super) message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RouteResultEnvelope {
    pub(super) job_id: String,
    pub(super) status: String,
    pub(super) result: Option<RouteResult>,
}

#[derive(Deserialize)]
pub(super) struct RouteResult {
    pub(super) routes: Vec<RouteResultItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RouteResultItem {
    pub(super) duration_seconds: Option<f64>,
}

#[derive(Deserialize)]
pub(super) struct ErrorEnvelope {
    pub(super) error: TcacheErrorBody,
}

pub(super) fn extract_travel_time(
    job_id: &str,
    from: &Location,
    to: &Location,
    envelope: RouteResultEnvelope,
) -> Result<TravelTime, RoutingError> {
    if envelope.job_id != job_id {
        return Err(invalid_result(
            job_id,
            from,
            to,
            "result jobId does not match",
        ));
    }
    if envelope.status != "completed" {
        return Err(invalid_result(
            job_id,
            from,
            to,
            "result status is not completed",
        ));
    }
    let seconds = envelope
        .result
        .and_then(|result| result.routes.into_iter().next())
        .and_then(|route| route.duration_seconds)
        .ok_or_else(|| invalid_result(job_id, from, to, "missing route durationSeconds"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(invalid_result(
            job_id,
            from,
            to,
            "invalid route durationSeconds",
        ));
    }
    let minutes = (seconds / 60.0).ceil();
    if minutes > f64::from(u32::MAX) {
        return Err(invalid_result(
            job_id,
            from,
            to,
            "duration exceeds supported range",
        ));
    }
    Ok(TravelTime {
        minutes: minutes as u32,
    })
}

fn invalid_result(job_id: &str, from: &Location, to: &Location, detail: &str) -> RoutingError {
    RoutingError::Provider(format!(
        "invalid tcache route result for job {job_id}{}: {detail}",
        pair_context(from, to)
    ))
}
