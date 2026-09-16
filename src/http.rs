use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::json;
use tokio::{sync::mpsc, task::JoinHandle, time::timeout};

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse},
    events::{
        ErrorEventData, JobEventContext, JobEventType, NoopOptimizationEventReporter,
        OptimizationErrorCode, OptimizationEventReporter, ProgressEventData, ProgressStage,
    },
    observation::{
        allowlisted_headers, JobObservationRecorder, JobTimelineEntry, ObservationDirection,
        ObservationHeaders, ObservationPeer,
    },
    routing::{RoutingError, RoutingProvider},
    schedule::ScheduleError,
    service::{OptimizationServiceError, RouteOptimizationService},
    solver::{RouteSolver, SolverError},
    trasolve::{TrasolveClient, TrasolveClientError, TrasolveHealthResponse},
};

const MAX_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const JOB_EVENT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

trait Optimizer: Send + Sync {
    fn optimize_with_reporter(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError>;
}

impl<P, S> Optimizer for RouteOptimizationService<P, S>
where
    P: RoutingProvider + Send + Sync,
    S: RouteSolver + Send + Sync,
{
    fn optimize_with_reporter(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        RouteOptimizationService::optimize_with_reporter(self, request, reporter)
    }
}

#[derive(Debug)]
enum QueuedJobEvent {
    Progress(ProgressEventData),
    Error(ErrorEventData),
    Result(OptimizeRouteResponse),
}

struct ChannelOptimizationEventReporter {
    sender: mpsc::UnboundedSender<QueuedJobEvent>,
}

impl ChannelOptimizationEventReporter {
    fn enqueue(&self, event: QueuedJobEvent) {
        if self.sender.send(event).is_err() {
            eprintln!("Trasolve job event callback queue is unavailable");
        }
    }
}

impl OptimizationEventReporter for ChannelOptimizationEventReporter {
    fn progress(&self, stage: ProgressStage, progress: u8, message: Option<&str>) {
        self.enqueue(QueuedJobEvent::Progress(ProgressEventData {
            stage,
            progress,
            message: message.map(str::to_owned),
        }));
    }

    fn error(&self, code: OptimizationErrorCode, message: &str, detail: &str) {
        self.enqueue(QueuedJobEvent::Error(ErrorEventData {
            code,
            message: message.to_owned(),
            detail: detail.to_owned(),
        }));
    }

    fn result(&self, response: &OptimizeRouteResponse) {
        self.enqueue(QueuedJobEvent::Result(response.clone()));
    }
}

async fn send_queued_job_event<T: Serialize>(
    client: &TrasolveClient,
    context: &mut JobEventContext,
    event_type: JobEventType,
    data: T,
) -> bool {
    let event = match context.next_event(event_type, data) {
        Ok(event) => event,
        Err(error) => {
            eprintln!("Trasolve job event sequence failed: {error}");
            return false;
        }
    };
    if let Err(error) = client.send_job_event(context.job_id(), &event).await {
        eprintln!(
            "Trasolve job event callback failed; sequence={}; type={:?}; error={error}",
            event.sequence, event.event_type
        );
    }
    true
}

struct JobCallbackSession {
    reporter: ChannelOptimizationEventReporter,
    sender_task: JoinHandle<()>,
}

impl JobCallbackSession {
    fn start(client: TrasolveClient, job_id: String) -> Option<Self> {
        let mut context = match JobEventContext::new(job_id) {
            Ok(context) => context,
            Err(error) => {
                eprintln!("Trasolve job callbacks disabled for invalid job_id: {error}");
                return None;
            }
        };
        let (sender, mut receiver) = mpsc::unbounded_channel::<QueuedJobEvent>();
        let sender_task = tokio::spawn(async move {
            while let Some(queued) = receiver.recv().await {
                let continue_sending = match queued {
                    QueuedJobEvent::Progress(data) => {
                        send_queued_job_event(&client, &mut context, JobEventType::Progress, data)
                            .await
                    }
                    QueuedJobEvent::Error(data) => {
                        send_queued_job_event(&client, &mut context, JobEventType::Error, data)
                            .await
                    }
                    QueuedJobEvent::Result(data) => {
                        send_queued_job_event(&client, &mut context, JobEventType::Result, data)
                            .await
                    }
                };
                if !continue_sending {
                    break;
                }
            }
        });
        Some(Self {
            reporter: ChannelOptimizationEventReporter { sender },
            sender_task,
        })
    }

    async fn finish(self) {
        let Self {
            reporter,
            mut sender_task,
        } = self;
        drop(reporter);

        match timeout(JOB_EVENT_DRAIN_TIMEOUT, &mut sender_task).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("Trasolve job event sender task failed: {error}"),
            Err(_) => {
                sender_task.abort();
                let _ = sender_task.await;
                eprintln!(
                    "Trasolve job event callback drain exceeded {} ms",
                    JOB_EVENT_DRAIN_TIMEOUT.as_millis()
                );
            }
        }
    }
}

#[derive(Clone)]
struct AppState {
    optimizer: Arc<dyn Optimizer>,
    trasolve: Option<TrasolveClient>,
    observation: Option<JobObservationRecorder>,
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
struct JobTimelineResponse {
    job_id: String,
    entries: Vec<JobTimelineEntry>,
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
    router_with_observation(optimizer, None, None)
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
    router_with_observation(optimizer, trasolve, None)
}

/// Builds the HTTP application with optional Trasolve and testbed observation support.
pub fn router_with_observation<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    trasolve: Option<TrasolveClient>,
    observation: Option<JobObservationRecorder>,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    let trasolve = trasolve.map(|client| match &observation {
        Some(observation) => client.with_observation(observation.clone()),
        None => client,
    });

    Router::new()
        .route("/health", get(health))
        .route("/optimize", post(optimize))
        .route(
            "/integration/trasolve/health",
            get(trasolve_integration_health),
        )
        .route("/integration/jobs/{job_id}/timeline", get(job_timeline))
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .with_state(AppState {
            optimizer: Arc::new(optimizer),
            trasolve,
            observation,
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
    let job_id = request.job_id.clone();
    let started = Instant::now();
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
    let callback = match &state.trasolve {
        Some(client) => JobCallbackSession::start(client.clone(), request.job_id.clone()),
        None => {
            println!("Trasolve job callbacks disabled: TRASOLVE_BASE_URL is not configured");
            None
        }
    };
    let noop_reporter = NoopOptimizationEventReporter;
    let reporter: &dyn OptimizationEventReporter = match &callback {
        Some(callback) => &callback.reporter,
        None => &noop_reporter,
    };
    let result = state.optimizer.optimize_with_reporter(request, reporter);

    if let Some(callback) = callback {
        callback.finish().await;
    }

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
    ) || (request.uri().path().starts_with("/integration/jobs/")
        && request.uri().path().ends_with("/timeline"))
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
