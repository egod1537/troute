use std::{
    convert::Infallible,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse},
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    events::NoopOptimizationEventReporter,
    failure::{
        rank_failure_suggestions, service_failure_detail, service_suggestions, FailureDetail,
        FailureSuggestion, DEFAULT_MAX_FAILURE_SUGGESTIONS,
    },
    jobs::{JobEventEnvelope, JobEventKind, JobExecutor, JobRunner},
    logging::logger,
    observation::{
        allowlisted_headers, JobObservationRecorder, JobTimelineEntry, ObservationDirection,
        ObservationHeaders, ObservationPeer,
    },
    routing::{RouteProviderSelectionDiagnostics, RoutingError, RoutingProvider},
    schedule::ScheduleError,
    service::{OptimizationServiceError, RouteOptimizationService},
    solver::{
        PortfolioCapabilities, PortfolioTelemetrySummary, RouteSolver, SolverError, TelemetryEvent,
    },
    storage::{JobIndexEntry, JobStatus, JobStore, JobStoreError, StoredJob},
};
use axum::{
    body::Body,
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Semaphore;

mod error;
mod handlers;
mod router;
mod validation_worker;
use error::{ApiError, ErrorEnvelope};
use router::enforce_routing_pair_limit;
pub use router::{
    router, router_with_observation, router_with_storage, router_with_storage_and_limit,
    router_with_storage_options, router_with_storage_runtime_options,
};

const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
pub const DEFAULT_SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
pub const DEFAULT_MAX_CONCURRENT_JOBS: usize = 4;
pub const DEFAULT_MAX_OUTSTANDING_JOBS: usize = 100;
pub const DEFAULT_MAX_CONCURRENT_MATRIX_PREVIEWS: usize = 2;
pub const DEFAULT_MAX_ROUTING_PAIRS: usize = 10_000;

/// Explicit process-level limits used when assembling the production router.
///
/// Compatibility router constructors still read their historical environment
/// variables. The server startup path supplies this value from its validated
/// application configuration so request handling never consults the process
/// environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterRuntimeOptions {
    pub max_concurrent_jobs: Option<usize>,
    pub max_outstanding_jobs: Option<usize>,
    pub max_concurrent_matrix_previews: usize,
    pub max_routing_pairs: usize,
    pub sse_heartbeat_interval: Duration,
    pub validation_cpu: Option<usize>,
    pub max_queued_validations: usize,
}

impl Default for RouterRuntimeOptions {
    fn default() -> Self {
        Self {
            max_concurrent_jobs: Some(DEFAULT_MAX_CONCURRENT_JOBS),
            max_outstanding_jobs: Some(DEFAULT_MAX_OUTSTANDING_JOBS),
            max_concurrent_matrix_previews: DEFAULT_MAX_CONCURRENT_MATRIX_PREVIEWS,
            max_routing_pairs: DEFAULT_MAX_ROUTING_PAIRS,
            sse_heartbeat_interval: DEFAULT_SSE_HEARTBEAT_INTERVAL,
            validation_cpu: None,
            max_queued_validations: 16,
        }
    }
}

#[derive(Clone)]
struct AppState {
    executor: Arc<dyn JobExecutor>,
    runner: Option<JobRunner>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
    sse_heartbeat_interval: Duration,
    matrix_permits: Arc<Semaphore>,
    max_routing_pairs: usize,
    validation_worker: validation_worker::ValidationWorker,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct JobTimelineResponse {
    job_id: String,
    entries: Vec<JobTimelineEntry>,
}

#[derive(Debug, Serialize)]
struct JobListResponse {
    jobs: Vec<JobIndexEntry>,
}

#[derive(Debug, Deserialize)]
struct JobListQuery {
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct SubmitJobResponse {
    job_id: String,
    status: JobStatus,
    created_at: i64,
}

#[derive(Debug, Serialize)]
struct CancelJobResponse {
    job_id: String,
    status: JobStatus,
}

#[derive(Debug, Serialize)]
struct MatrixPreviewResponse {
    travel_time_matrix: Vec<Vec<Option<u32>>>,
}

#[derive(Debug, Deserialize)]
struct SolverTelemetryQuery {
    after: Option<u64>,
    limit: Option<usize>,
}

#[derive(Debug, Serialize)]
struct SolverTelemetryResponse {
    schema_version: u16,
    job_id: String,
    collected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<PortfolioTelemetrySummary>,
    events: Vec<TelemetryEvent>,
    next_after: u64,
    has_more: bool,
}

fn enforce_portfolio_experiment(
    state: &AppState,
    request: &OptimizeRouteRequest,
) -> Result<(), ApiError> {
    let Some(experiment) = &request.portfolio_experiment else {
        return Ok(());
    };
    let capabilities = state.executor.solver_capabilities().ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "PORTFOLIO_EXPERIMENT_UNAVAILABLE",
            "Portfolio experiments are unavailable.",
            "the configured solver has no portfolio registry",
        )
    })?;
    if !capabilities.experiment_override_enabled {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "PORTFOLIO_EXPERIMENT_DISABLED",
            "Per-request portfolio experiments are disabled.",
            "set TROUTE_PORTFOLIO_EXPERIMENT_ENABLED=true to opt in",
        ));
    }
    if experiment
        .total_budget_ms
        .is_some_and(|budget| budget == 0 || budget > capabilities.limits.max_total_budget_ms)
        || experiment
            .logical_cpu_slots
            .is_some_and(|slots| slots == 0 || slots > capabilities.limits.max_logical_cpu_slots)
    {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "PORTFOLIO_EXPERIMENT_LIMIT_EXCEEDED",
            "Portfolio experiment limits were exceeded.",
            format!(
                "maximum budget={} ms; maximum logical slots={}",
                capabilities.limits.max_total_budget_ms, capabilities.limits.max_logical_cpu_slots
            ),
        ));
    }
    if let Some(requested) = &experiment.solver_ids {
        let registered: std::collections::HashMap<_, _> = capabilities
            .solvers
            .iter()
            .map(|solver| (&solver.id, solver.unavailable_reason.as_deref()))
            .collect();
        let unique: std::collections::HashSet<_> = requested.iter().collect();
        if requested.is_empty()
            || unique.len() != requested.len()
            || requested
                .iter()
                .any(|solver| !registered.contains_key(solver))
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "INVALID_PORTFOLIO_SOLVER_SET",
                "The requested Solver set is invalid.",
                "Solver IDs must be registered, unique, and non-empty",
            ));
        }
        if let Some(unavailable) = requested
            .iter()
            .find(|solver| registered.get(*solver).copied().flatten().is_some())
        {
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "PORTFOLIO_SOLVER_UNAVAILABLE",
                "A requested Solver is unavailable in this deployment.",
                format!(
                    "{}: {}",
                    unavailable,
                    registered
                        .get(unavailable)
                        .copied()
                        .flatten()
                        .unwrap_or("unavailable")
                ),
            ));
        }
    }
    Ok(())
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
    if request.uri().path() == "/validate"
        || request.uri().path() == "/optimize"
        || (request.uri().path().starts_with("/integration/jobs/")
            && request.uri().path().ends_with("/cancel"))
    {
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
    } else if request.uri().path() == "/integration/jobs" {
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("GET,HEAD,POST"));
    } else if request.uri().path() == "/health"
        || request.uri().path().starts_with("/integration/jobs/")
    {
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
mod tests;
