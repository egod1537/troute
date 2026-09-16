use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse},
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    events::{OptimizationErrorCode, OptimizationEventReporter, ProgressStage},
    observation::{
        allowlisted_headers, JobObservationRecorder, JobTimelineEntry, ObservationDirection,
        ObservationHeaders, ObservationPeer,
    },
    routing::{RoutingError, RoutingProvider},
    schedule::ScheduleError,
    service::{OptimizationServiceError, RouteOptimizationService},
    solver::{RouteSolver, SolverError},
    storage::{
        JobIndexEntry, JobStatus, JobStore, JobStoreError, StoredJob, StoredJobError,
        TerminalWriteOutcome,
    },
};
use axum::{
    body::Body,
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;

trait Optimizer: Send + Sync {
    fn optimize_with_cancellation(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError>;
}

impl<P, S> Optimizer for RouteOptimizationService<P, S>
where
    P: RoutingProvider + Send + Sync,
    S: RouteSolver + Send + Sync,
{
    fn optimize_with_cancellation(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        RouteOptimizationService::optimize_with_cancellation(self, request, reporter, cancellation)
    }
}

struct PersistentOptimizationEventReporter<'a> {
    job_id: &'a str,
    store: Option<&'a Arc<dyn JobStore>>,
    cancellation: &'a CancellationToken,
}

impl PersistentOptimizationEventReporter<'_> {
    fn log_failure(&self, operation: &str, result: Result<(), JobStoreError>) {
        if let Err(error) = result {
            eprintln!(
                "job persistence failed; job_id={}; operation={operation}; error={error}",
                self.job_id
            );
        }
    }
}

impl OptimizationEventReporter for PersistentOptimizationEventReporter<'_> {
    fn progress(&self, stage: ProgressStage, progress: u8, message: Option<&str>) {
        if let Some(store) = self.store {
            if let Err(error) = store.update_progress(self.job_id, stage, progress, message) {
                if matches!(
                    error,
                    JobStoreError::InvalidTransition {
                        status: JobStatus::Cancelled,
                        ..
                    }
                ) {
                    self.cancellation.cancel();
                    return;
                }
                self.log_failure("update_progress", Err(error));
            }
        }
    }

    fn error(&self, code: OptimizationErrorCode, message: &str, detail: &str) {
        if let Some(store) = self.store {
            let code = serde_json::to_value(code)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_else(|| "INTERNAL_ERROR".to_owned());
            match store.save_error(
                self.job_id,
                &StoredJobError {
                    code,
                    message: message.to_owned(),
                    detail: detail.to_owned(),
                },
            ) {
                Ok(TerminalWriteOutcome::Applied) => {}
                Ok(TerminalWriteOutcome::Cancelled) => {
                    self.cancellation.cancel();
                }
                Err(error) => self.log_failure("save_error", Err(error)),
            }
        }
    }

    fn result(&self, response: &OptimizeRouteResponse) {
        if let Some(store) = self.store {
            match store.save_result(self.job_id, response) {
                Ok(TerminalWriteOutcome::Applied) => {}
                Ok(TerminalWriteOutcome::Cancelled) => {
                    self.cancellation.cancel();
                }
                Err(error) => self.log_failure("save_result", Err(error)),
            }
        }
    }
}

#[derive(Clone, Default)]
struct JobCancellationRegistry {
    jobs: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl JobCancellationRegistry {
    fn reserve(&self, job_id: &str, token: CancellationToken) -> bool {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if jobs.contains_key(job_id) {
            return false;
        }
        jobs.insert(job_id.to_owned(), token);
        true
    }

    fn get(&self, job_id: &str) -> Option<CancellationToken> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(job_id)
            .cloned()
    }

    fn remove(&self, job_id: &str) {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(job_id);
    }
}

#[derive(Clone)]
struct AppState {
    optimizer: Arc<dyn Optimizer>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
    cancellations: JobCancellationRegistry,
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
struct CancelJobResponse {
    job_id: String,
    status: JobStatus,
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    reason: String,
    detail: Option<String>,
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
            detail: None,
        }
    }

    fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
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
            OptimizationServiceError::Cancelled => Self::new(
                StatusCode::CONFLICT,
                "JOB_CANCELLED",
                "The optimization job was cancelled.",
                "cancellation was requested before optimization completed",
            ),
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

    fn from_storage(error: JobStoreError) -> Self {
        match error {
            JobStoreError::DuplicateJob(job_id) => Self::new(
                StatusCode::CONFLICT,
                "DUPLICATE_JOB_ID",
                "A job with this job_id already exists.",
                job_id,
            ),
            source => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "STORAGE_ERROR",
                "Job records could not be stored.",
                source.to_string(),
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
                    detail: self.detail,
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
    router_with_storage(optimizer, None, None)
}

/// Builds the HTTP application with optional testbed observation support.
pub fn router_with_observation<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    observation: Option<JobObservationRecorder>,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    router_with_storage(optimizer, observation, None)
}

/// Builds the HTTP application with persistent job and timeline storage.
pub fn router_with_storage<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    Router::new()
        .route("/health", get(health))
        .route("/optimize", post(optimize))
        .route("/integration/jobs", get(list_jobs))
        .route("/integration/jobs/{job_id}", get(get_job))
        .route("/integration/jobs/{job_id}/cancel", post(cancel_job))
        .route("/integration/jobs/{job_id}/timeline", get(job_timeline))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .with_state(AppState {
            optimizer: Arc::new(optimizer),
            observation,
            job_store,
            cancellations: JobCancellationRegistry::default(),
        })
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn optimize(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<OptimizeRouteRequest>, JsonRejection>,
) -> Result<Json<OptimizeRouteResponse>, ApiError> {
    let Json(request) = request.map_err(ApiError::from_json_rejection)?;
    OptimizationProblem::try_from(request.clone())
        .map_err(OptimizationServiceError::InvalidRequest)
        .map_err(ApiError::from_service)?;
    let job_id = request.job_id.clone();
    let started = Instant::now();
    let cancellation = CancellationToken::new();
    if !state.cancellations.reserve(&job_id, cancellation.clone()) {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "DUPLICATE_JOB_ID",
            "A job with this job_id is already running.",
            &job_id,
        ));
    }
    if let Some(store) = &state.job_store {
        if let Err(error) = store.create_job(&request) {
            state.cancellations.remove(&job_id);
            return Err(ApiError::from_storage(error));
        }
        if let Err(error) = store.mark_running(&job_id) {
            if matches!(
                error,
                JobStoreError::InvalidTransition {
                    status: JobStatus::Cancelled,
                    ..
                }
            ) {
                cancellation.cancel();
            } else {
                state.cancellations.remove(&job_id);
                return Err(ApiError::from_storage(error));
            }
        }
    }
    let pair_id = state
        .observation
        .as_ref()
        .map(|observation| observation.next_pair_id("opt"));
    if let (Some(observation), Some(pair_id)) = (&state.observation, &pair_id) {
        let mut entry = observation.entry(
            pair_id,
            ObservationDirection::Request,
            ObservationPeer::Testbed,
            ObservationPeer::Troute,
        );
        entry.method = Some("POST".to_owned());
        entry.path = Some("/optimize".to_owned());
        entry.headers = allowlisted_headers(&headers);
        entry.body = serde_json::to_value(&request).ok();
        observation.append(&job_id, entry);
    }
    let reporter = PersistentOptimizationEventReporter {
        job_id: &job_id,
        store: state.job_store.as_ref(),
        cancellation: &cancellation,
    };
    let result = state
        .optimizer
        .optimize_with_cancellation(request, &reporter, &cancellation);

    state.cancellations.remove(&job_id);

    match result {
        Ok(response) => {
            record_optimize_response(
                state.observation.as_ref(),
                &job_id,
                pair_id.as_deref(),
                started,
                StatusCode::OK,
                serde_json::to_value(&response).ok(),
            );
            Ok(Json(response))
        }
        Err(error) => {
            let error = ApiError::from_service(error);
            let body = json!({
                "error": {
                    "code": error.code,
                    "message": error.message,
                }
            });
            record_optimize_response(
                state.observation.as_ref(),
                &job_id,
                pair_id.as_deref(),
                started,
                error.status,
                Some(body),
            );
            Err(error)
        }
    }
}

async fn list_jobs(
    State(state): State<AppState>,
    Query(query): Query<JobListQuery>,
) -> Result<Json<JobListResponse>, ApiError> {
    let limit = query.limit.unwrap_or(50).min(500);
    let jobs = match &state.job_store {
        Some(store) => store.list_recent(limit).map_err(ApiError::from_storage)?,
        None => Vec::new(),
    };
    Ok(Json(JobListResponse { jobs }))
}

async fn get_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<StoredJob>, ApiError> {
    let started = Instant::now();
    let job = match &state.job_store {
        Some(store) => store.get_job(&job_id).map_err(ApiError::from_storage)?,
        None => None,
    };
    let job = job.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "The requested job does not exist.",
            job_id.as_str(),
        )
    })?;
    record_job_inspection(state.observation.as_ref(), &job_id, &headers, started, &job);
    Ok(Json(job))
}

fn record_job_inspection(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    headers: &HeaderMap,
    started: Instant,
    job: &StoredJob,
) {
    let Some(observation) = observation else {
        return;
    };
    let pair_id = observation.next_pair_id("inspect");
    let mut request = observation.entry(
        &pair_id,
        ObservationDirection::Request,
        ObservationPeer::Testbed,
        ObservationPeer::Troute,
    );
    request.method = Some("GET".to_owned());
    request.path = Some(format!("/integration/jobs/{job_id}"));
    request.headers = allowlisted_headers(headers);
    observation.append(job_id, request);

    let mut response = observation.entry(
        &pair_id,
        ObservationDirection::Response,
        ObservationPeer::Troute,
        ObservationPeer::Testbed,
    );
    response.status = Some(StatusCode::OK.as_u16());
    response.latency_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
    response.headers = Some(ObservationHeaders::from([(
        header::CONTENT_TYPE.as_str().to_owned(),
        "application/json".to_owned(),
    )]));
    response.body = serde_json::to_value(job).ok();
    observation.append(job_id, response);
}

async fn cancel_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<CancelJobResponse>, ApiError> {
    const CANCEL_MESSAGE: &str = "Job cancelled by request";

    let started = Instant::now();
    let pair_id = state
        .observation
        .as_ref()
        .map(|observation| observation.next_pair_id("cancel"));
    let request_entry = match (&state.observation, &pair_id) {
        (Some(observation), Some(pair_id)) => {
            let mut entry = observation.entry(
                pair_id,
                ObservationDirection::Request,
                ObservationPeer::Testbed,
                ObservationPeer::Troute,
            );
            entry.method = Some("POST".to_owned());
            entry.path = Some(format!("/integration/jobs/{job_id}/cancel"));
            entry.headers = allowlisted_headers(&headers);
            Some(entry)
        }
        _ => None,
    };

    let store = state.job_store.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "Job not found.",
            &job_id,
        )
    })?;
    // Hold a clone across the persisted transition so optimize cleanup cannot
    // remove the token in the result-vs-cancel race.
    let active = state.cancellations.get(&job_id);

    match store.cancel_job(&job_id, CANCEL_MESSAGE) {
        Ok(job) => {
            if let Some(active) = active {
                active.cancel();
            }
            let response = CancelJobResponse {
                job_id: job_id.clone(),
                status: job.status,
            };
            record_cancel_exchange(
                state.observation.as_ref(),
                &job_id,
                pair_id.as_deref(),
                request_entry,
                started,
                StatusCode::OK,
                serde_json::to_value(&response).ok(),
            );
            Ok(Json(response))
        }
        Err(JobStoreError::JobNotFound(_)) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "Job not found.",
            &job_id,
        )),
        Err(JobStoreError::JobNotCancellable { status, .. }) => {
            let error = ApiError::new(
                StatusCode::CONFLICT,
                "JOB_NOT_CANCELLABLE",
                "The job is already terminal.",
                format!("job status is {}", status.as_str()),
            )
            .with_detail(status.as_str());
            let body = json!({
                "error": {
                    "code": error.code,
                    "message": error.message,
                    "detail": error.detail,
                }
            });
            record_cancel_exchange(
                state.observation.as_ref(),
                &job_id,
                pair_id.as_deref(),
                request_entry,
                started,
                StatusCode::CONFLICT,
                Some(body),
            );
            Err(error)
        }
        Err(error) => Err(ApiError::from_storage(error)),
    }
}

#[allow(clippy::too_many_arguments)]
fn record_cancel_exchange(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    pair_id: Option<&str>,
    request: Option<JobTimelineEntry>,
    started: Instant,
    status: StatusCode,
    body: Option<serde_json::Value>,
) {
    let (Some(observation), Some(pair_id), Some(request)) = (observation, pair_id, request) else {
        return;
    };
    observation.append(job_id, request);
    let mut response = observation.entry(
        pair_id,
        ObservationDirection::Response,
        ObservationPeer::Troute,
        ObservationPeer::Testbed,
    );
    response.status = Some(status.as_u16());
    response.latency_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
    response.headers = Some(ObservationHeaders::from([(
        header::CONTENT_TYPE.as_str().to_owned(),
        "application/json".to_owned(),
    )]));
    response.body = body;
    observation.append(job_id, response);
}

fn record_optimize_response(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    pair_id: Option<&str>,
    started: Instant,
    status: StatusCode,
    body: Option<serde_json::Value>,
) {
    let (Some(observation), Some(pair_id)) = (observation, pair_id) else {
        return;
    };
    let mut entry = observation.entry(
        pair_id,
        ObservationDirection::Response,
        ObservationPeer::Troute,
        ObservationPeer::Testbed,
    );
    entry.status = Some(status.as_u16());
    entry.latency_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
    entry.headers = Some(ObservationHeaders::from([(
        header::CONTENT_TYPE.as_str().to_owned(),
        "application/json".to_owned(),
    )]));
    entry.body = body;
    observation.append(job_id, entry);
}

async fn job_timeline(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Json<JobTimelineResponse> {
    let mut indexed_entries = state
        .observation
        .as_ref()
        .map(|observation| observation.list(&job_id))
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .collect::<Vec<_>>();
    indexed_entries.sort_by(|(left_index, left), (right_index, right)| {
        left.timestamp_ms
            .cmp(&right.timestamp_ms)
            .then_with(|| left_index.cmp(right_index))
    });
    let entries = indexed_entries
        .into_iter()
        .map(|(_, entry)| entry)
        .collect();

    Json(JobTimelineResponse { job_id, entries })
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
    if request.uri().path() == "/optimize"
        || (request.uri().path().starts_with("/integration/jobs/")
            && request.uri().path().ends_with("/cancel"))
    {
        response
            .headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
    } else if matches!(request.uri().path(), "/health" | "/integration/jobs")
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
            "job_id":"route-http-test",
            "locations":[
                {
                    "id":"place-1",
                    "place_id":"GOOGLE_PLACE_ID_1",
                    "open_time":"09:00",
                    "close_time":"18:00",
                    "stay_minutes":0
                },
                {
                    "id":"place-2",
                    "place_id":"GOOGLE_PLACE_ID_2",
                    "open_time":"09:00",
                    "close_time":"18:00",
                    "stay_minutes":0
                }
            ],
            "start_time":"09:00"
        }"#
    }
}
