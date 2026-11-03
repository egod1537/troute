use super::super::*;

pub(in crate::http) async fn build_matrix(
    State(state): State<AppState>,
    request: Result<Json<OptimizeRouteRequest>, JsonRejection>,
) -> Result<Json<MatrixPreviewResponse>, ApiError> {
    let request = prepare_matrix_request(&state, request)?;
    let matrix = execute_matrix_preview(&state, request).await?;

    Ok(Json(MatrixPreviewResponse {
        travel_time_matrix: matrix.into_rows(),
    }))
}

fn prepare_matrix_request(
    state: &AppState,
    request: Result<Json<OptimizeRouteRequest>, JsonRejection>,
) -> Result<OptimizeRouteRequest, ApiError> {
    let Json(mut request) = request.map_err(ApiError::from_json_rejection)?;
    // Matrix preview ignores an editor-supplied matrix. Euclidean mode remains
    // enabled so coordinate previews stay local and avoid provider queries.
    request.travel_time_matrix = None;
    request.sparse_travel_time_matrix = None;
    OptimizationProblem::try_from(request.clone())
        .map_err(OptimizationServiceError::InvalidRequest)
        .map_err(ApiError::from_service)?;
    enforce_routing_pair_limit(&request, state.max_routing_pairs)?;
    Ok(request)
}

async fn execute_matrix_preview(
    state: &AppState,
    request: OptimizeRouteRequest,
) -> Result<crate::matrix::TravelTimeMatrix, ApiError> {
    let _permit = state
        .matrix_permits
        .clone()
        .try_acquire_owned()
        .map_err(|_| {
            ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "MATRIX_CAPACITY_EXCEEDED",
                "Too many matrix previews are running.",
                "matrix preview concurrency limit reached",
            )
        })?;
    let executor = state.executor.clone();
    tokio::task::spawn_blocking(move || executor.build_travel_time_matrix(request))
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "MATRIX_EXECUTION_FAILED",
                "Travel-time matrix generation failed unexpectedly.",
                error.to_string(),
            )
        })?
        .map_err(ApiError::from_service)
}
