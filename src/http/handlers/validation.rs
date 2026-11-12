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
    if let Some(response) = ValidateRouteResponse::unsupported(&request) {
        return Ok(Json(response));
    }
    enforce_routing_pair_limit(&request.route, state.max_routing_pairs)?;
    let cancellation = CancellationToken::new();
    let _guard = CancelValidation(cancellation.clone());
    if request.check_feasibility {
        let result = state
            .validation_worker
            .submit(request, cancellation)
            .map_err(|error| {
                use crate::http::validation_worker::QueueError;
                match error {
                    QueueError::Full => ApiError::new(
                        StatusCode::TOO_MANY_REQUESTS,
                        "FAST_VALIDATION_CAPACITY_EXCEEDED",
                        "Too many fast validation requests are waiting.",
                        "the dedicated validation queue is full",
                    ),
                    QueueError::Unavailable => ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "FAST_VALIDATION_UNAVAILABLE",
                        "Fast validation is temporarily unavailable.",
                        "the dedicated validation worker is unavailable",
                    ),
                }
            })?;
        return await_validation(result).await;
    }
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
    let executor = state.executor.clone();
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        executor.validate_route(request, &cancellation)
    });
    await_validation(task).await
}

async fn await_validation<E: std::fmt::Display>(
    task: impl std::future::Future<
        Output = Result<Result<ValidateRouteResponse, OptimizationServiceError>, E>,
    >,
) -> Result<Json<ValidateRouteResponse>, ApiError> {
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
