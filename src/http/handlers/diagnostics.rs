use super::super::*;

pub(in crate::http) async fn provider_policy(
    State(state): State<AppState>,
) -> Result<Json<RouteProviderSelectionDiagnostics>, ApiError> {
    state
        .executor
        .provider_policy_diagnostics()
        .map(Json)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "PROVIDER_POLICY_UNAVAILABLE",
                "Route provider policy diagnostics are unavailable.",
                "the configured routing provider has no policy resolver",
            )
        })
}

pub(in crate::http) async fn solver_capabilities(
    State(state): State<AppState>,
) -> Result<Json<PortfolioCapabilities>, ApiError> {
    state
        .executor
        .solver_capabilities()
        .map(Json)
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "SOLVER_CAPABILITIES_UNAVAILABLE",
                "Solver capabilities are unavailable.",
                "the configured solver does not expose a portfolio registry",
            )
        })
}
