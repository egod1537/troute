use super::super::*;
use crate::validation::{ValidateRouteRequest, ValidateRouteResponse};

pub(in crate::http) async fn validate_route(
    State(state): State<AppState>,
    request: Result<Json<ValidateRouteRequest>, JsonRejection>,
) -> Result<Json<ValidateRouteResponse>, ApiError> {
    let Json(mut request) = request.map_err(ApiError::from_json_rejection)?;
    state.executor.apply_request_defaults(&mut request.route);
    if let Err(response) = crate::validation::validate_input(&request) {
        return Ok(Json(response));
    }
    enforce_routing_pair_limit(&request.route, state.max_routing_pairs)?;
    let permit = state
        .matrix_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "VALIDATION_CAPACITY_EXCEEDED",
                "Too many validation requests are running.",
                "validation shares the bounded matrix capacity",
            )
        })?;
    let cancellation = CancellationToken::new();
    let _guard = CancelValidation(cancellation.clone());
    let executor = state.executor.clone();
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        executor.validate_route(request, &cancellation)
    });
    let response = tokio::time::timeout(Duration::from_secs(30), task)
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "VALIDATION_TIMEOUT",
                "Route validation timed out.",
                "validation exceeded 30 seconds",
            )
        })?
        .map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "VALIDATION_FAILED",
                "Route validation failed unexpectedly.",
                error.to_string(),
            )
        })?
        .map_err(ApiError::from_service)?;
    Ok(Json(response))
}

struct CancelValidation(CancellationToken);
impl Drop for CancelValidation {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
