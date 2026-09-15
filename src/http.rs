use std::sync::Arc;

use axum::{
    body::Body,
    extract::{rejection::JsonRejection, DefaultBodyLimit, State},
    http::{header, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse},
    routing::{RoutingError, RoutingProvider},
    schedule::ScheduleError,
    service::{OptimizationServiceError, RouteOptimizationService},
    solver::{RouteSolver, SolverError},
    trasolve::{TrasolveClient, TrasolveClientError, TrasolveHealthResponse},
};

const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

trait Optimizer: Send + Sync {
    fn optimize(
        &self,
        request: OptimizeRouteRequest,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError>;
}

impl<P, S> Optimizer for RouteOptimizationService<P, S>
where
    P: RoutingProvider + Send + Sync,
    S: RouteSolver + Send + Sync,
{
    fn optimize(
        &self,
        request: OptimizeRouteRequest,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        RouteOptimizationService::optimize(self, request)
    }
}

#[derive(Clone)]
struct AppState {
    optimizer: Arc<dyn Optimizer>,
    trasolve: Option<TrasolveClient>,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct TrasolveIntegrationHealthResponse {
    status: &'static str,
    trasolve: TrasolveHealthResponse,
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    reason: String,
}

impl ApiError {
    fn new(
        status: StatusCode,
        code: &'static str,
        message: &'static str,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            status,
            code,
            message,
            reason: reason.into(),
        }
    }

    fn from_json_rejection(rejection: JsonRejection) -> Self {
        let status = rejection.status();
        if status == StatusCode::UNSUPPORTED_MEDIA_TYPE {
            return Self::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "UNSUPPORTED_MEDIA_TYPE",
                "Content-Type must be application/json.",
                rejection.body_text(),
            );
        }
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            return Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "REQUEST_TOO_LARGE",
                "Request body must not exceed 1 MiB.",
                rejection.body_text(),
            );
        }
        Self::new(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Request body must be valid JSON matching the optimize contract.",
            rejection.body_text(),
        )
    }

    fn from_service(error: OptimizationServiceError) -> Self {
        match error {
            OptimizationServiceError::InvalidRequest(source) => Self::new(
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST",
                "The optimize request is invalid.",
                source.to_string(),
            ),
            OptimizationServiceError::Routing(source) => match source {
                RoutingError::Provider(reason) => Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ROUTING_UNAVAILABLE",
                    "Travel-time routing is temporarily unavailable.",
                    reason,
                ),
            },
            OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "NO_FEASIBLE_ROUTE",
                "No feasible route was found.",
                "solver found no feasible route",
            ),
            OptimizationServiceError::Schedule(
                source @ (ScheduleError::OutsideSingleDay
                | ScheduleError::TimeWindowViolation { .. }),
            ) => Self::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "NO_FEASIBLE_ROUTE",
                "No feasible route was found.",
                source.to_string(),
            ),
            OptimizationServiceError::Solver(source) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "Route optimization failed unexpectedly.",
                source.to_string(),
            ),
            OptimizationServiceError::Schedule(source) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "Route scheduling failed unexpectedly.",
                source.to_string(),
            ),
        }
    }

    fn trasolve_not_configured() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "TRASOLVE_NOT_CONFIGURED",
            "Trasolve integration is not configured.",
            "TRASOLVE_BASE_URL is not set",
        )
    }

    fn from_trasolve(error: TrasolveClientError) -> Self {
        match error {
            TrasolveClientError::Configuration(reason) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "TRASOLVE_NOT_CONFIGURED",
                "Trasolve integration is not configured.",
                reason,
            ),
            TrasolveClientError::Timeout => Self::new(
                StatusCode::GATEWAY_TIMEOUT,
                "TRASOLVE_TIMEOUT",
                "Trasolve did not respond before the timeout.",
                error.to_string(),
            ),
            TrasolveClientError::Connection(reason) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "TRASOLVE_UNAVAILABLE",
                "Trasolve is unavailable.",
                reason,
            ),
            TrasolveClientError::UpstreamHttp { status, body: _ } => Self::new(
                if status >= 500 {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::BAD_GATEWAY
                },
                "TRASOLVE_UPSTREAM_ERROR",
                "Trasolve returned an unsuccessful response.",
                format!("Trasolve returned HTTP {status}"),
            ),
            TrasolveClientError::InvalidResponse(reason) => Self::new(
                StatusCode::BAD_GATEWAY,
                "TRASOLVE_INVALID_RESPONSE",
                "Trasolve returned an invalid response.",
                reason,
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        eprintln!(
            "HTTP request failed; code={}; status={}; reason={}",
            self.code,
            self.status.as_u16(),
            self.reason
        );
        (
            self.status,
            Json(ErrorEnvelope {
                error: ErrorBody {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

/// Builds the HTTP application around an existing optimization service.
pub fn router<P, S>(optimizer: RouteOptimizationService<P, S>) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    router_with_trasolve(optimizer, None)
}

/// Builds the HTTP application with an optional reusable Trasolve client.
pub fn router_with_trasolve<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    trasolve: Option<TrasolveClient>,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    Router::new()
        .route("/health", get(health))
        .route("/optimize", post(optimize))
        .route(
            "/integration/trasolve/health",
            get(trasolve_integration_health),
        )
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .with_state(AppState {
            optimizer: Arc::new(optimizer),
            trasolve,
        })
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn optimize(
    State(state): State<AppState>,
    request: Result<Json<OptimizeRouteRequest>, JsonRejection>,
) -> Result<Json<OptimizeRouteResponse>, ApiError> {
    let Json(request) = request.map_err(ApiError::from_json_rejection)?;
    state
        .optimizer
        .optimize(request)
        .map(Json)
        .map_err(ApiError::from_service)
}

async fn trasolve_integration_health(
    State(state): State<AppState>,
) -> Result<Json<TrasolveIntegrationHealthResponse>, ApiError> {
    let client = state
        .trasolve
        .as_ref()
        .ok_or_else(ApiError::trasolve_not_configured)?;
    let trasolve = client.health().await.map_err(ApiError::from_trasolve)?;
    Ok(Json(TrasolveIntegrationHealthResponse {
        status: "ok",
        trasolve,
    }))
}

async fn method_not_allowed<B>(request: Request<B>) -> Response {
    let mut response = ApiError::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "METHOD_NOT_ALLOWED",
        "The requested HTTP method is not allowed for this endpoint.",
        format!(
            "method {} is not allowed for {}",
            request.method(),
            request.uri()
        ),
    )
    .into_response();
    if request.uri().path() == "/optimize" {
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
    } else if matches!(
        request.uri().path(),
        "/health" | "/integration/trasolve/health"
    ) {
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET,HEAD"));
    }
    response
}

async fn not_found(request: Request<Body>) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "The requested endpoint does not exist.",
        format!("no route for {} {}", request.method(), request.uri()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::Location,
        matrix::TravelTimeMatrix,
        routing::{RoutingError, RoutingProvider},
        solver::{RouteSolver, SolverError, SolverInput, SolverSolution},
    };
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    struct FailingRoutingProvider;

    impl RoutingProvider for FailingRoutingProvider {
        fn travel_time_matrix(
            &self,
            _locations: &[Location],
        ) -> Result<TravelTimeMatrix, RoutingError> {
            Err(RoutingError::Provider("provider is offline".to_owned()))
        }
    }

    struct UnusedSolver;

    impl RouteSolver for UnusedSolver {
        fn solve(&self, _input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
            unreachable!("routing failure must stop the pipeline before solving")
        }
    }

    struct FixedRoutingProvider;

    impl RoutingProvider for FixedRoutingProvider {
        fn travel_time_matrix(
            &self,
            _locations: &[Location],
        ) -> Result<TravelTimeMatrix, RoutingError> {
            TravelTimeMatrix::new(vec![vec![0]])
                .map_err(|error| RoutingError::Provider(error.to_string()))
        }
    }

    struct NoFeasibleSolver;

    impl RouteSolver for NoFeasibleSolver {
        fn solve(&self, _input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
            Err(SolverError::NoFeasibleRoute)
        }
    }

    #[tokio::test]
    async fn routing_failures_have_a_stable_json_mapping() {
        let app = router(RouteOptimizationService::new(
            FailingRoutingProvider,
            UnusedSolver,
        ));
        let request = Request::builder()
            .method("POST")
            .uri("/optimize")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(valid_request()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let json: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(json["error"]["code"], "ROUTING_UNAVAILABLE");
        assert_eq!(
            json["error"]["message"],
            "Travel-time routing is temporarily unavailable."
        );
    }

    #[tokio::test]
    async fn no_feasible_solver_result_has_a_stable_json_mapping() {
        let app = router(RouteOptimizationService::new(
            FixedRoutingProvider,
            NoFeasibleSolver,
        ));
        let request = Request::builder()
            .method("POST")
            .uri("/optimize")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(valid_request()))
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(json["error"]["code"], "NO_FEASIBLE_ROUTE");
        assert_eq!(json["error"]["message"], "No feasible route was found.");
    }

    fn valid_request() -> &'static str {
        r#"{
            "locations":[{
                "id":"place-1",
                "place_id":"GOOGLE_PLACE_ID",
                "open_time":"09:00",
                "close_time":"18:00",
                "stay_minutes":60
            }],
            "start_location_id":"place-1",
            "start_time":"09:00"
        }"#
    }
}
