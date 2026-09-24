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
    jobs::{JobEventData, JobEventKind, JobExecutor, JobRunner},
    observation::{
        allowlisted_headers, JobObservationRecorder, JobTimelineEntry, ObservationDirection,
        ObservationHeaders, ObservationPeer,
    },
    routing::{RoutingError, RoutingProvider},
    schedule::ScheduleError,
    service::{OptimizationServiceError, RouteOptimizationService},
    solver::{RouteSolver, SolverError},
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

const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const DEFAULT_SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone)]
struct AppState {
    executor: Arc<dyn JobExecutor>,
    runner: Option<JobRunner>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
    sse_heartbeat_interval: Duration,
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
    travel_time_matrix: Vec<Vec<u32>>,
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
                source @ RoutingError::MatrixBuild { .. } => Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ROUTING_UNAVAILABLE",
                    "Travel-time routing is temporarily unavailable.",
                    source.to_string(),
                ),
                other => Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ROUTING_MATRIX_ERROR",
                    "Travel-time routing returned an invalid matrix.",
                    other.to_string(),
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
    router_with_storage_options(
        optimizer,
        observation,
        job_store,
        max_concurrent_jobs_from_env(),
        DEFAULT_SSE_HEARTBEAT_INTERVAL,
    )
}

/// Builds the HTTP application with an explicit background-job concurrency limit.
pub fn router_with_storage_and_limit<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
    max_concurrent_jobs: Option<usize>,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    router_with_storage_options(
        optimizer,
        observation,
        job_store,
        max_concurrent_jobs,
        DEFAULT_SSE_HEARTBEAT_INTERVAL,
    )
}

/// Builds the HTTP application with explicit background-job and SSE settings.
pub fn router_with_storage_options<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
    max_concurrent_jobs: Option<usize>,
    sse_heartbeat_interval: Duration,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    let executor: Arc<dyn JobExecutor> = Arc::new(optimizer);
    let runner = job_store
        .as_ref()
        .map(|store| JobRunner::new(executor.clone(), store.clone(), max_concurrent_jobs));
    Router::new()
        .route("/health", get(health))
        .route("/optimize", post(optimize))
        .route("/integration/jobs", get(list_jobs).post(submit_job))
        .route("/integration/matrix", post(build_matrix))
        .route("/integration/jobs/{job_id}", get(get_job))
        .route("/integration/jobs/{job_id}/events", get(job_events))
        .route("/integration/jobs/{job_id}/cancel", post(cancel_job))
        .route("/integration/jobs/{job_id}/timeline", get(job_timeline))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .with_state(AppState {
            executor,
            runner,
            observation,
            job_store,
            sse_heartbeat_interval: sse_heartbeat_interval.max(Duration::from_millis(1)),
        })
}

fn max_concurrent_jobs_from_env() -> Option<usize> {
    let value = std::env::var("TROUTE_MAX_CONCURRENT_JOBS").ok()?;
    match value.parse::<usize>() {
        Ok(limit) if limit > 0 => Some(limit),
        _ => {
            eprintln!(
                "ignoring invalid TROUTE_MAX_CONCURRENT_JOBS={value:?}; expected a positive integer"
            );
            None
        }
    }
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn build_matrix(
    State(state): State<AppState>,
    request: Result<Json<OptimizeRouteRequest>, JsonRejection>,
) -> Result<Json<MatrixPreviewResponse>, ApiError> {
    let Json(mut request) = request.map_err(ApiError::from_json_rejection)?;
    request.travel_time_matrix = None;
    OptimizationProblem::try_from(request.clone())
        .map_err(OptimizationServiceError::InvalidRequest)
        .map_err(ApiError::from_service)?;
    let executor = state.executor.clone();
    let matrix = tokio::task::spawn_blocking(move || executor.build_travel_time_matrix(request))
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "MATRIX_EXECUTION_FAILED",
                "Travel-time matrix generation failed unexpectedly.",
                error.to_string(),
            )
        })?
        .map_err(ApiError::from_service)?;
    Ok(Json(MatrixPreviewResponse {
        travel_time_matrix: matrix.into_rows(),
    }))
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
    let pair_id = state
        .observation
        .as_ref()
        .map(|observation| observation.next_pair_id("opt"));

    let result = if let Some(runner) = &state.runner {
        let submission = runner
            .submit_with_completion(request.clone())
            .map_err(ApiError::from_storage)?;
        record_optimize_request(
            state.observation.as_ref(),
            &job_id,
            pair_id.as_deref(),
            &headers,
            &request,
        );
        submission
            .completion
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "JOB_EXECUTION_FAILED",
                    "Route optimization failed unexpectedly.",
                    "background job ended without reporting completion",
                )
            })?
            .map_err(|reason| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "STORAGE_ERROR",
                    "Job records could not be stored.",
                    reason,
                )
            })?;
        let job = runner
            .store()
            .get_job(&job_id)
            .map_err(ApiError::from_storage)?
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "STORAGE_ERROR",
                    "Job records could not be stored.",
                    format!("completed job disappeared: {job_id}"),
                )
            })?;
        legacy_result_from_job(job)
    } else {
        record_optimize_request(
            state.observation.as_ref(),
            &job_id,
            pair_id.as_deref(),
            &headers,
            &request,
        );
        let executor = state.executor.clone();
        let cancellation = CancellationToken::new();
        tokio::task::spawn_blocking(move || {
            executor.execute(request, &NoopOptimizationEventReporter, &cancellation)
        })
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "JOB_EXECUTION_FAILED",
                "Route optimization failed unexpectedly.",
                error.to_string(),
            )
        })?
    };

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

fn record_optimize_request(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    pair_id: Option<&str>,
    headers: &HeaderMap,
    request: &OptimizeRouteRequest,
) {
    let (Some(observation), Some(pair_id)) = (observation, pair_id) else {
        return;
    };
    let mut entry = observation.entry(
        pair_id,
        ObservationDirection::Request,
        ObservationPeer::Testbed,
        ObservationPeer::Troute,
    );
    entry.method = Some("POST".to_owned());
    entry.path = Some("/optimize".to_owned());
    entry.headers = allowlisted_headers(headers);
    entry.body = serde_json::to_value(request).ok();
    observation.append(job_id, entry);
}

async fn submit_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<OptimizeRouteRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(request) = request.map_err(ApiError::from_json_rejection)?;
    OptimizationProblem::try_from(request.clone())
        .map_err(OptimizationServiceError::InvalidRequest)
        .map_err(ApiError::from_service)?;
    let runner = state.runner.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "JOB_STORE_UNAVAILABLE",
            "Asynchronous job submission is unavailable.",
            "the application was built without a job store",
        )
    })?;
    let job_id = request.job_id.clone();
    let started = Instant::now();
    let pair_id = state
        .observation
        .as_ref()
        .map(|observation| observation.next_pair_id("submit"));
    let submitted_state = runner
        .submit(request.clone())
        .map_err(ApiError::from_storage)?;
    if let (Some(observation), Some(pair_id)) = (&state.observation, &pair_id) {
        let mut entry = observation.entry(
            pair_id,
            ObservationDirection::Request,
            ObservationPeer::Testbed,
            ObservationPeer::Troute,
        );
        entry.method = Some("POST".to_owned());
        entry.path = Some("/integration/jobs".to_owned());
        entry.headers = allowlisted_headers(&headers);
        entry.body = serde_json::to_value(&request).ok();
        observation.append(&job_id, entry);
    }

    let response = SubmitJobResponse {
        job_id: submitted_state.job_id,
        status: submitted_state.status,
        created_at: submitted_state.created_at,
    };
    record_submit_response(
        state.observation.as_ref(),
        &job_id,
        pair_id.as_deref(),
        started,
        &response,
    );
    Ok((StatusCode::ACCEPTED, Json(response)))
}

fn legacy_result_from_job(
    job: StoredJob,
) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
    match job.state.status {
        JobStatus::Completed => job.result.ok_or_else(|| {
            OptimizationServiceError::Solver(SolverError::Failed(
                "completed job has no persisted result".to_owned(),
            ))
        }),
        JobStatus::Cancelled => Err(OptimizationServiceError::Cancelled),
        JobStatus::Failed => {
            let error = job.error.ok_or_else(|| {
                OptimizationServiceError::Solver(SolverError::Failed(
                    "failed job has no persisted error".to_owned(),
                ))
            })?;
            match error.code.as_str() {
                "NO_FEASIBLE_ROUTE" => Err(OptimizationServiceError::Solver(
                    SolverError::NoFeasibleRoute,
                )),
                "ROUTING_UNAVAILABLE" => Err(OptimizationServiceError::Routing(
                    RoutingError::Provider(error.detail),
                )),
                _ => Err(OptimizationServiceError::Solver(SolverError::Failed(
                    error.detail,
                ))),
            }
        }
        status => Err(OptimizationServiceError::Solver(SolverError::Failed(
            format!("background job completion reported non-terminal status {status:?}"),
        ))),
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

async fn job_events(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Response, ApiError> {
    let store = state.job_store.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "The requested job does not exist.",
            &job_id,
        )
    })?;

    // Subscribe before reading the source-of-truth snapshot. An event racing
    // with the read is then either represented by the snapshot or buffered for
    // live delivery (and can harmlessly be observed in both forms).
    let subscription = state
        .runner
        .as_ref()
        .and_then(|runner| runner.subscribe(&job_id));
    let initial = store
        .get_job(&job_id)
        .map_err(ApiError::from_storage)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "JOB_NOT_FOUND",
                "The requested job does not exist.",
                &job_id,
            )
        })?;
    let initial_terminal = !matches!(
        initial.state.status,
        JobStatus::Pending | JobStatus::Running
    );
    let persisted_sequence = u64::try_from(initial.state.updated_at).unwrap_or(0);
    let initial_sequence = subscription
        .as_ref()
        .map_or(persisted_sequence, |subscription| subscription.cursor);
    let snapshot = JobEventData::snapshot(initial);
    let event_store = store.clone();
    let event_job_id = job_id.clone();

    let stream = async_stream::stream! {
        yield Ok::<Event, Infallible>(sse_event("snapshot", initial_sequence, &snapshot));
        if initial_terminal {
            return;
        }

        let Some(mut subscription) = subscription else {
            return;
        };
        let mut cursor = initial_sequence;
        loop {
            match subscription.receiver.recv().await {
                Ok(event) => {
                    if event.sequence <= cursor {
                        continue;
                    }
                    cursor = event.sequence;
                    let terminal = matches!(
                        event.kind,
                        JobEventKind::Completed | JobEventKind::Failed | JobEventKind::Cancelled
                    );
                    yield Ok(sse_event(event.kind.as_str(), event.sequence, &event.data));
                    if terminal {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    match event_store.get_job(&event_job_id) {
                        Ok(Some(job)) => {
                            let terminal = !matches!(
                                job.state.status,
                                JobStatus::Pending | JobStatus::Running
                            );
                            cursor = subscription.current_sequence();
                            let snapshot = JobEventData::snapshot(job);
                            yield Ok(sse_event("snapshot", cursor, &snapshot));
                            if terminal {
                                break;
                            }
                        }
                        Ok(None) => break,
                        Err(error) => {
                            eprintln!(
                                "SSE lag recovery failed; job_id={event_job_id}; error={error}"
                            );
                            break;
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(state.sse_heartbeat_interval)
                .text("heartbeat"),
        )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    response.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    Ok(response)
}

fn sse_event(event: &'static str, sequence: u64, data: &JobEventData) -> Event {
    Event::default()
        .event(event)
        .id(sequence.to_string())
        .data(serde_json::to_string(data).expect("job event data must serialize"))
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
    let active = state
        .runner
        .as_ref()
        .and_then(|runner| runner.activity(&job_id));

    match store.cancel_job(&job_id, CANCEL_MESSAGE) {
        Ok(job) => {
            if let Some(active) = active {
                active.publish_persisted(store.as_ref(), &job_id, JobEventKind::Cancelled);
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

fn record_submit_response(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    pair_id: Option<&str>,
    started: Instant,
    body: &SubmitJobResponse,
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
    entry.status = Some(StatusCode::ACCEPTED.as_u16());
    entry.latency_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
    entry.headers = Some(ObservationHeaders::from([(
        header::CONTENT_TYPE.as_str().to_owned(),
        "application/json".to_owned(),
    )]));
    entry.body = serde_json::to_value(body).ok();
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
mod tests {
    use super::*;
    use crate::{
        domain::Location,
        matrix::TravelTimeMatrix,
        routing::{RoutingContext, RoutingError, RoutingProvider},
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
            _context: &RoutingContext,
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
            _context: &RoutingContext,
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
