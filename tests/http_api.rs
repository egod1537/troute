use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::time::sleep;
use tower::ServiceExt;
use troute::{
    development::{DevelopmentRouteSolver, DevelopmentRoutingProvider},
    http,
    observation::{
        JobObservationRecorder, JobTimelineEntry, JobTimelineStore, ObservationDirection,
        ObservationPeer,
    },
    solver::{RouteSolver, SolverError, SolverInput, SolverSolution},
    storage::{FileJobStore, FileJobTimelineStore, JobStatus, JobStore},
    RouteOptimizationService,
};

#[derive(Clone)]
struct SlowCancellationSolver {
    started: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

#[derive(Clone)]
struct GatedSolver {
    started: Arc<AtomicUsize>,
    release: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    max_active: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct GatedFailureSolver {
    started: Arc<AtomicBool>,
    release: Arc<AtomicBool>,
}

#[derive(Clone)]
struct TimedSolver {
    duration: Duration,
    finished_at: Arc<Mutex<Option<std::time::Instant>>>,
}

impl RouteSolver for TimedSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        let started = std::time::Instant::now();
        while started.elapsed() < self.duration {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        *self.finished_at.lock().unwrap() = Some(std::time::Instant::now());
        Ok(SolverSolution {
            visit_order: (0..input.problem.locations().len()).collect(),
        })
    }
}

impl RouteSolver for GatedFailureSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        self.started.store(true, Ordering::Release);
        while !self.release.load(Ordering::Acquire) {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(SolverError::NoFeasibleRoute)
    }
}

impl RouteSolver for GatedSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        self.started.fetch_add(1, Ordering::AcqRel);
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(active, Ordering::AcqRel);
        let started = std::time::Instant::now();
        let outcome = loop {
            if input.cancellation.is_cancelled() {
                break Err(SolverError::Cancelled);
            }
            if self.release.load(Ordering::Acquire) {
                break Ok(SolverSolution {
                    visit_order: (0..input.problem.locations().len()).collect(),
                });
            }
            if started.elapsed() > Duration::from_secs(5) {
                break Err(SolverError::Failed(
                    "test solver release timed out".to_owned(),
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        self.active.fetch_sub(1, Ordering::AcqRel);
        outcome
    }
}

impl RouteSolver for SlowCancellationSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        self.started.store(true, Ordering::Release);
        let started = std::time::Instant::now();
        while !input.cancellation.is_cancelled() {
            if started.elapsed() > Duration::from_secs(5) {
                return Err(SolverError::Failed(
                    "test solver was not cancelled before timeout".to_owned(),
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.finished.store(true, Ordering::Release);
        Err(SolverError::Cancelled)
    }
}

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "troute-http-storage-test-{}-{}",
            std::process::id(),
            NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn app() -> Router {
    http::router(RouteOptimizationService::new(
        DevelopmentRoutingProvider,
        DevelopmentRouteSolver,
    ))
}

fn app_with_observation() -> (Router, JobObservationRecorder) {
    let observation = JobObservationRecorder::in_memory();
    let app = http::router_with_observation(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        Some(observation.clone()),
    );
    (app, observation)
}

fn app_with_file_storage(data_dir: &std::path::Path) -> Router {
    let observation =
        JobObservationRecorder::new(Arc::new(FileJobTimelineStore::new(data_dir).unwrap()));
    http::router_with_storage(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        Some(observation),
        Some(Arc::new(FileJobStore::new(data_dir).unwrap())),
    )
}

fn valid_request() -> Value {
    json!({
        "job_id": "route-http-integration-test",
        "locations": [
            {
                "id": "place-1",
                "place_id": "GOOGLE_PLACE_ID_1",
                "open_time": "09:00",
                "close_time": "18:00",
                "stay_minutes": 60
            },
            {
                "id": "place-2",
                "place_id": "GOOGLE_PLACE_ID_2",
                "open_time": "09:00",
                "close_time": "18:00",
                "stay_minutes": 30
            },
            {
                "id": "place-3",
                "place_id": "GOOGLE_PLACE_ID_3",
                "open_time": "09:00",
                "close_time": "18:00",
                "stay_minutes": 0
            }
        ],
        "start_time": "09:00"
    })
}

fn request_with_job_id(job_id: &str) -> Value {
    let mut request = valid_request();
    request["job_id"] = json!(job_id);
    request
}

fn request_with_debug_duration(job_id: &str, duration_ms: u64) -> Value {
    let mut request = request_with_job_id(job_id);
    request["debug"] = json!({ "min_job_duration_ms": duration_ms });
    request
}

fn request_with_debug_shuffle(job_id: &str, duration_ms: u64) -> Value {
    let mut request = request_with_job_id(job_id);
    request["locations"] = json!([
        {"id":"place-1","place_id":"GOOGLE_PLACE_ID_1","open_time":"00:00","close_time":"23:59","stay_minutes":0},
        {"id":"place-2","place_id":"GOOGLE_PLACE_ID_2","open_time":"00:00","close_time":"23:59","stay_minutes":2},
        {"id":"place-3","place_id":"GOOGLE_PLACE_ID_3","open_time":"00:00","close_time":"23:59","stay_minutes":3},
        {"id":"place-4","place_id":"GOOGLE_PLACE_ID_4","open_time":"00:00","close_time":"23:59","stay_minutes":4},
        {"id":"place-5","place_id":"GOOGLE_PLACE_ID_5","open_time":"00:00","close_time":"23:59","stay_minutes":0}
    ]);
    request["debug"] = json!({
        "min_job_duration_ms": duration_ms,
        "shuffle_result_route": true,
        "shuffle_seed": 1_234
    });
    request
}

fn gated_solver() -> GatedSolver {
    GatedSolver {
        started: Arc::new(AtomicUsize::new(0)),
        release: Arc::new(AtomicBool::new(false)),
        active: Arc::new(AtomicUsize::new(0)),
        max_active: Arc::new(AtomicUsize::new(0)),
    }
}

async fn wait_for_solver_starts(started: &AtomicUsize, expected: usize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while started.load(Ordering::Acquire) < expected {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

async fn wait_for_job_status(store: &dyn JobStore, job_id: &str, expected: JobStatus) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if store
                .get_job(job_id)
                .unwrap()
                .is_some_and(|job| job.state.status == expected)
            {
                break;
            }
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

async fn send(method: Method, path: &str, content_type: Option<&str>, body: String) -> Response {
    send_to(app(), method, path, content_type, body).await
}

async fn send_to(
    app: Router,
    method: Method,
    path: &str,
    content_type: Option<&str>,
    body: String,
) -> Response {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    send_request_to(app, builder.body(Body::from(body)).unwrap()).await
}

async fn send_request_to(app: Router, request: Request<Body>) -> Response {
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap();
    Response {
        status,
        headers,
        body,
    }
}

struct Response {
    status: StatusCode,
    headers: axum::http::HeaderMap,
    body: Value,
}

async fn open_event_stream(app: Router, job_id: &str) -> (axum::http::HeaderMap, Body) {
    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/integration/jobs/{job_id}/events"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    (headers, response.into_body())
}

async fn next_sse_message(body: &mut Body, buffered: &mut String) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(end) = buffered.find("\n\n") {
                return Some(buffered.drain(..end + 2).collect());
            }
            let frame = body.frame().await?;
            let frame = frame.expect("SSE body frame must be readable");
            if let Ok(data) = frame.into_data() {
                buffered.push_str(
                    std::str::from_utf8(&data).expect("SSE events must contain UTF-8 data"),
                );
            }
        }
    })
    .await
    .expect("SSE event timed out")
}

fn sse_field<'a>(message: &'a str, name: &str) -> Option<&'a str> {
    message.lines().find_map(|line| {
        line.strip_prefix(name)
            .and_then(|value| value.strip_prefix(':'))
            .map(str::trim)
    })
}

fn sse_data(message: &str) -> Value {
    serde_json::from_str(sse_field(message, "data").expect("SSE data field is missing")).unwrap()
}

#[tokio::test]
async fn health_remains_available() {
    let response = send(Method::GET, "/health", None, String::new()).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, json!({ "status": "ok" }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_submit_returns_202_before_solver_and_persists_state_transitions() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let solver = gated_solver();
    let app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(DevelopmentRoutingProvider, solver.clone()),
        None,
        Some(store.clone()),
        None,
    );

    let submitted = tokio::time::timeout(
        Duration::from_secs(1),
        send_to(
            app.clone(),
            Method::POST,
            "/integration/jobs",
            Some("application/json"),
            request_with_job_id("async-state-flow").to_string(),
        ),
    )
    .await
    .expect("submission must not wait for the gated solver");
    assert_eq!(submitted.status, StatusCode::ACCEPTED);
    assert_eq!(submitted.body["job_id"], "async-state-flow");
    assert_eq!(submitted.body["status"], "pending");
    assert!(submitted.body["created_at"].is_number());
    assert_eq!(submitted.body.as_object().unwrap().len(), 3);

    wait_for_solver_starts(&solver.started, 1).await;
    let running = send_to(
        app.clone(),
        Method::GET,
        "/integration/jobs/async-state-flow",
        None,
        String::new(),
    )
    .await;
    assert_eq!(running.status, StatusCode::OK);
    assert_eq!(running.body["status"], "running");
    assert_eq!(running.body["stage"], "solving");
    assert_eq!(running.body["progress"], 60);
    assert!(running.body["result"].is_null());

    solver.release.store(true, Ordering::Release);
    wait_for_job_status(store.as_ref(), "async-state-flow", JobStatus::Completed).await;
    let completed = send_to(
        app.clone(),
        Method::GET,
        "/integration/jobs/async-state-flow",
        None,
        String::new(),
    )
    .await;
    assert_eq!(completed.body["status"], "completed");
    assert_eq!(completed.body["progress"], 100);
    assert!(completed.body["result"].is_object());
    assert!(completed.body["error"].is_null());

    let duplicate = send_to(
        app,
        Method::POST,
        "/integration/jobs",
        Some("application/json"),
        request_with_job_id("async-state-flow").to_string(),
    )
    .await;
    assert_error(duplicate, StatusCode::CONFLICT, "DUPLICATE_JOB_ID");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_minimum_duration_spaces_real_stages_persists_request_and_preserves_result() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        None,
        Some(store.clone()),
        None,
    );

    let started = std::time::Instant::now();
    let submitted = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs",
        Some("application/json"),
        request_with_debug_duration("debug-paced", 1_000).to_string(),
    )
    .await;
    assert_eq!(submitted.status, StatusCode::ACCEPTED);

    let (_, mut body) = open_event_stream(app.clone(), "debug-paced").await;
    let mut buffered = String::new();
    let mut stages = Vec::new();
    loop {
        let message = next_sse_message(&mut body, &mut buffered)
            .await
            .expect("paced job must reach a terminal event");
        let event = sse_field(&message, "event");
        if matches!(event, Some("snapshot" | "progress")) {
            if let Some(stage) = sse_data(&message)["stage"].as_str() {
                if stages.last().map(String::as_str) != Some(stage) {
                    stages.push(stage.to_owned());
                }
            }
        }
        if event == Some("completed") {
            break;
        }
    }
    assert!(started.elapsed() >= Duration::from_millis(1_000));
    let expected = ["accepted", "building_matrix", "solving", "scheduling"];
    let first = expected
        .iter()
        .position(|stage| Some(*stage) == stages.first().map(String::as_str))
        .expect("snapshot must expose a real progress stage");
    assert_eq!(stages, expected[first..]);
    assert_eq!(stages.last().map(String::as_str), Some("scheduling"));

    let paced = store.get_job("debug-paced").unwrap().unwrap();
    assert_eq!(
        paced.request.debug.unwrap().min_job_duration_ms,
        Some(1_000)
    );
    let persisted_request: Value = serde_json::from_str(
        &fs::read_to_string(temporary.0.join("jobs/debug-paced/request.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted_request["debug"]["min_job_duration_ms"], 1_000);

    let baseline_started = std::time::Instant::now();
    assert_eq!(
        send_to(
            app,
            Method::POST,
            "/integration/jobs",
            Some("application/json"),
            request_with_job_id("debug-baseline").to_string(),
        )
        .await
        .status,
        StatusCode::ACCEPTED
    );
    wait_for_job_status(store.as_ref(), "debug-baseline", JobStatus::Completed).await;
    assert!(baseline_started.elapsed() < Duration::from_millis(750));
    let baseline = store.get_job("debug-baseline").unwrap().unwrap();
    assert_eq!(paced.result, baseline.result);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_shuffle_combines_with_pacing_and_persists_one_result_for_sse_and_get() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        None,
        Some(store.clone()),
        None,
    );
    let request = request_with_debug_shuffle("debug-shuffled", 100);
    let started = std::time::Instant::now();

    let submitted = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs",
        Some("application/json"),
        request.to_string(),
    )
    .await;
    assert_eq!(submitted.status, StatusCode::ACCEPTED);

    let (_, mut body) = open_event_stream(app.clone(), "debug-shuffled").await;
    let mut buffered = String::new();
    let completed_snapshot = loop {
        let message = next_sse_message(&mut body, &mut buffered)
            .await
            .expect("debug shuffle job must emit a completed event");
        if sse_field(&message, "event") == Some("completed") {
            break sse_data(&message);
        }
    };
    assert!(started.elapsed() >= Duration::from_millis(100));

    let detail = send_to(
        app,
        Method::GET,
        "/integration/jobs/debug-shuffled",
        None,
        String::new(),
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(detail.body["status"], "completed");
    assert_eq!(completed_snapshot["result"], detail.body["result"]);

    let persisted_request: Value = serde_json::from_str(
        &fs::read_to_string(temporary.0.join("jobs/debug-shuffled/request.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted_request, request);
    let persisted_result: Value = serde_json::from_str(
        &fs::read_to_string(temporary.0.join("jobs/debug-shuffled/result.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(persisted_result, detail.body["result"]);

    let route = detail.body["result"]["route"].as_array().unwrap();
    assert_eq!(route.first().unwrap()["location_id"], "place-1");
    assert_eq!(route.last().unwrap()["location_id"], "place-5");
    assert_ne!(
        route[1..4]
            .iter()
            .map(|stop| stop["location_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["place-2", "place-3", "place-4"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_minimum_adds_no_terminal_delay_to_an_already_slow_solver() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let finished_at = Arc::new(Mutex::new(None));
    let app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(
            DevelopmentRoutingProvider,
            TimedSolver {
                duration: Duration::from_millis(1_500),
                finished_at: finished_at.clone(),
            },
        ),
        None,
        Some(store.clone()),
        None,
    );
    let started = std::time::Instant::now();
    assert_eq!(
        send_to(
            app,
            Method::POST,
            "/integration/jobs",
            Some("application/json"),
            request_with_debug_duration("debug-slow-solver", 1_000).to_string(),
        )
        .await
        .status,
        StatusCode::ACCEPTED
    );
    wait_for_job_status(store.as_ref(), "debug-slow-solver", JobStatus::Completed).await;
    let observed_terminal = std::time::Instant::now();
    let solver_finished = finished_at.lock().unwrap().unwrap();
    assert!(started.elapsed() >= Duration::from_millis(1_500));
    assert!(observed_terminal.duration_since(solver_finished) < Duration::from_millis(250));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_during_debug_pacing_is_immediate() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        None,
        Some(store.clone()),
        None,
    );
    assert_eq!(
        send_to(
            app.clone(),
            Method::POST,
            "/integration/jobs",
            Some("application/json"),
            request_with_debug_duration("debug-cancel", 5_000).to_string(),
        )
        .await
        .status,
        StatusCode::ACCEPTED
    );
    wait_for_job_status(store.as_ref(), "debug-cancel", JobStatus::Running).await;
    sleep(Duration::from_millis(50)).await;

    let cancel_started = std::time::Instant::now();
    let cancelled = send_to(
        app,
        Method::POST,
        "/integration/jobs/debug-cancel/cancel",
        None,
        String::new(),
    )
    .await;
    assert_eq!(cancelled.status, StatusCode::OK);
    assert!(cancel_started.elapsed() < Duration::from_millis(300));
    let job = store.get_job("debug-cancel").unwrap().unwrap();
    assert_eq!(job.state.status, JobStatus::Cancelled);
    assert!(job.result.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pending_queue_time_counts_toward_debug_minimum() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let solver = gated_solver();
    let app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(DevelopmentRoutingProvider, solver.clone()),
        None,
        Some(store.clone()),
        Some(1),
    );
    assert_eq!(
        send_to(
            app.clone(),
            Method::POST,
            "/integration/jobs",
            Some("application/json"),
            request_with_job_id("queue-blocker").to_string(),
        )
        .await
        .status,
        StatusCode::ACCEPTED
    );
    wait_for_solver_starts(&solver.started, 1).await;

    let queued_at = std::time::Instant::now();
    assert_eq!(
        send_to(
            app,
            Method::POST,
            "/integration/jobs",
            Some("application/json"),
            request_with_debug_duration("queue-paced", 200).to_string(),
        )
        .await
        .status,
        StatusCode::ACCEPTED
    );
    sleep(Duration::from_millis(300)).await;
    assert_eq!(
        store.get_job("queue-paced").unwrap().unwrap().state.status,
        JobStatus::Pending
    );

    let released_at = std::time::Instant::now();
    solver.release.store(true, Ordering::Release);
    wait_for_job_status(store.as_ref(), "queue-paced", JobStatus::Completed).await;
    assert!(queued_at.elapsed() >= Duration::from_millis(300));
    assert!(released_at.elapsed() < Duration::from_millis(250));
}

#[tokio::test]
async fn excessive_debug_duration_is_rejected_before_job_creation() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        None,
        Some(store.clone()),
        None,
    );
    let response = send_to(
        app,
        Method::POST,
        "/integration/jobs",
        Some("application/json"),
        request_with_debug_duration("debug-too-long", 60_001).to_string(),
    )
    .await;
    assert_error(response, StatusCode::BAD_REQUEST, "INVALID_REQUEST");
    assert!(store.get_job("debug-too-long").unwrap().is_none());
}

#[tokio::test]
async fn legacy_optimize_reuses_debug_pacing_for_success_and_failure() {
    let temporary = TestDirectory::new();
    let app = app_with_file_storage(&temporary.0);
    let started = std::time::Instant::now();
    let response = send_to(
        app.clone(),
        Method::POST,
        "/optimize",
        Some("application/json"),
        request_with_debug_duration("debug-legacy-success", 300).to_string(),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);
    assert!(started.elapsed() >= Duration::from_millis(300));

    let mut failed_request = request_with_debug_duration("debug-legacy-failure", 300);
    failed_request["locations"][1]["close_time"] = json!("09:10");
    let failed_started = std::time::Instant::now();
    let failed = send_to(
        app,
        Method::POST,
        "/optimize",
        Some("application/json"),
        failed_request.to_string(),
    )
    .await;
    assert_error(
        failed,
        StatusCode::UNPROCESSABLE_ENTITY,
        "NO_FEASIBLE_ROUTE",
    );
    assert!(failed_started.elapsed() >= Duration::from_millis(300));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_reconnects_with_snapshot_orders_progress_and_closes_after_completion() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let solver = gated_solver();
    let app = http::router_with_storage_options(
        RouteOptimizationService::new(DevelopmentRoutingProvider, solver.clone()),
        None,
        Some(store.clone()),
        None,
        Duration::from_secs(15),
    );
    let submitted = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs",
        Some("application/json"),
        request_with_job_id("sse-completed").to_string(),
    )
    .await;
    assert_eq!(submitted.status, StatusCode::ACCEPTED);
    wait_for_solver_starts(&solver.started, 1).await;

    let (_, mut disconnected_body) = open_event_stream(app.clone(), "sse-completed").await;
    let mut disconnected_buffer = String::new();
    let first_snapshot = next_sse_message(&mut disconnected_body, &mut disconnected_buffer)
        .await
        .unwrap();
    assert_eq!(sse_field(&first_snapshot, "event"), Some("snapshot"));
    assert_eq!(sse_data(&first_snapshot)["status"], "running");
    drop(disconnected_body);

    let (headers, mut body) = open_event_stream(app.clone(), "sse-completed").await;
    assert_eq!(headers[header::CONTENT_TYPE], "text/event-stream");
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    assert_eq!(headers[header::CONNECTION], "keep-alive");
    assert_eq!(headers["x-accel-buffering"], "no");
    let mut buffered = String::new();
    let snapshot = next_sse_message(&mut body, &mut buffered).await.unwrap();
    assert_eq!(sse_field(&snapshot, "event"), Some("snapshot"));
    let snapshot_id: u64 = sse_field(&snapshot, "id").unwrap().parse().unwrap();
    let snapshot_data = sse_data(&snapshot);
    assert_eq!(snapshot_data["job_id"], "sse-completed");
    assert_eq!(snapshot_data["status"], "running");
    assert_eq!(snapshot_data["stage"], "solving");
    assert_eq!(snapshot_data["request"]["job_id"], "sse-completed");

    solver.release.store(true, Ordering::Release);
    let mut last_id = snapshot_id;
    let mut saw_progress = false;
    loop {
        let message = next_sse_message(&mut body, &mut buffered)
            .await
            .expect("terminal SSE event must be sent");
        let Some(event) = sse_field(&message, "event") else {
            continue;
        };
        let id: u64 = sse_field(&message, "id").unwrap().parse().unwrap();
        assert!(id > last_id);
        last_id = id;
        match event {
            "progress" => {
                saw_progress = true;
                assert!(sse_data(&message)["progress"].as_u64().unwrap() >= 85);
            }
            "completed" => {
                let data = sse_data(&message);
                assert_eq!(data["status"], "completed");
                assert!(data["result"].is_object());
                break;
            }
            unexpected => panic!("unexpected SSE event: {unexpected}"),
        }
    }
    assert!(saw_progress);
    assert!(next_sse_message(&mut body, &mut buffered).await.is_none());

    let (_, mut late_body) = open_event_stream(app, "sse-completed").await;
    let mut late_buffer = String::new();
    let late_snapshot = next_sse_message(&mut late_body, &mut late_buffer)
        .await
        .unwrap();
    assert_eq!(sse_field(&late_snapshot, "event"), Some("snapshot"));
    assert_eq!(sse_data(&late_snapshot)["status"], "completed");
    assert!(next_sse_message(&mut late_body, &mut late_buffer)
        .await
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_sends_heartbeat_and_cancelled_terminal_event() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let started = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let app = http::router_with_storage_options(
        RouteOptimizationService::new(
            DevelopmentRoutingProvider,
            SlowCancellationSolver {
                started: started.clone(),
                finished: finished.clone(),
            },
        ),
        None,
        Some(store.clone()),
        None,
        Duration::from_millis(10),
    );
    let submitted = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs",
        Some("application/json"),
        request_with_job_id("sse-cancelled").to_string(),
    )
    .await;
    assert_eq!(submitted.status, StatusCode::ACCEPTED);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !started.load(Ordering::Acquire) {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();

    let (_, mut body) = open_event_stream(app.clone(), "sse-cancelled").await;
    let mut buffered = String::new();
    assert_eq!(
        sse_field(
            &next_sse_message(&mut body, &mut buffered).await.unwrap(),
            "event"
        ),
        Some("snapshot")
    );
    loop {
        let message = next_sse_message(&mut body, &mut buffered).await.unwrap();
        if message.lines().any(|line| line == ": heartbeat") {
            break;
        }
    }

    let cancelled = send_to(
        app,
        Method::POST,
        "/integration/jobs/sse-cancelled/cancel",
        None,
        String::new(),
    )
    .await;
    assert_eq!(cancelled.status, StatusCode::OK);
    loop {
        let message = next_sse_message(&mut body, &mut buffered)
            .await
            .expect("cancelled event must be sent");
        if sse_field(&message, "event") == Some("cancelled") {
            assert_eq!(sse_data(&message)["status"], "cancelled");
            break;
        }
    }
    assert!(next_sse_message(&mut body, &mut buffered).await.is_none());
    tokio::time::timeout(Duration::from_secs(2), async {
        while !finished.load(Ordering::Acquire) {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sse_failed_event_contains_persisted_error_and_closes() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(
            DevelopmentRoutingProvider,
            GatedFailureSolver {
                started: started.clone(),
                release: release.clone(),
            },
        ),
        None,
        Some(store),
        None,
    );
    assert_eq!(
        send_to(
            app.clone(),
            Method::POST,
            "/integration/jobs",
            Some("application/json"),
            request_with_job_id("sse-failed").to_string(),
        )
        .await
        .status,
        StatusCode::ACCEPTED
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while !started.load(Ordering::Acquire) {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();

    let (_, mut body) = open_event_stream(app, "sse-failed").await;
    let mut buffered = String::new();
    assert_eq!(
        sse_field(
            &next_sse_message(&mut body, &mut buffered).await.unwrap(),
            "event"
        ),
        Some("snapshot")
    );
    release.store(true, Ordering::Release);
    loop {
        let message = next_sse_message(&mut body, &mut buffered)
            .await
            .expect("failed event must be sent");
        if sse_field(&message, "event") == Some("failed") {
            let data = sse_data(&message);
            assert_eq!(data["status"], "failed");
            assert_eq!(data["error"]["code"], "NO_FEASIBLE_ROUTE");
            break;
        }
    }
    assert!(next_sse_message(&mut body, &mut buffered).await.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_jobs_run_concurrently_and_respect_an_explicit_limit() {
    for (limit, expected_started_before_release, expected_max) in
        [(None, 2usize, 2usize), (Some(1usize), 1usize, 1usize)]
    {
        let temporary = TestDirectory::new();
        let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
        let solver = gated_solver();
        let app = http::router_with_storage_and_limit(
            RouteOptimizationService::new(DevelopmentRoutingProvider, solver.clone()),
            None,
            Some(store.clone()),
            limit,
        );

        for job_id in ["concurrent-a", "concurrent-b"] {
            let response = send_to(
                app.clone(),
                Method::POST,
                "/integration/jobs",
                Some("application/json"),
                request_with_job_id(job_id).to_string(),
            )
            .await;
            assert_eq!(response.status, StatusCode::ACCEPTED);
        }
        wait_for_solver_starts(&solver.started, expected_started_before_release).await;
        if limit.is_some() {
            sleep(Duration::from_millis(25)).await;
            assert_eq!(solver.started.load(Ordering::Acquire), 1);
            assert_eq!(
                store.get_job("concurrent-b").unwrap().unwrap().state.status,
                JobStatus::Pending
            );
        }

        solver.release.store(true, Ordering::Release);
        wait_for_job_status(store.as_ref(), "concurrent-a", JobStatus::Completed).await;
        wait_for_job_status(store.as_ref(), "concurrent-b", JobStatus::Completed).await;
        assert_eq!(solver.max_active.load(Ordering::Acquire), expected_max);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn async_submit_persists_failure_and_can_cancel_a_running_job() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        None,
        Some(store.clone()),
        None,
    );
    let mut failed_request = request_with_job_id("async-failure");
    failed_request["locations"][1]["close_time"] = json!("09:10");
    let submitted = send_to(
        app,
        Method::POST,
        "/integration/jobs",
        Some("application/json"),
        failed_request.to_string(),
    )
    .await;
    assert_eq!(submitted.status, StatusCode::ACCEPTED);
    wait_for_job_status(store.as_ref(), "async-failure", JobStatus::Failed).await;
    let failed = store.get_job("async-failure").unwrap().unwrap();
    assert_eq!(failed.error.unwrap().code, "NO_FEASIBLE_ROUTE");
    assert!(failed.result.is_none());

    let cancel_directory = TestDirectory::new();
    let cancel_store = Arc::new(FileJobStore::new(&cancel_directory.0).unwrap());
    let started = Arc::new(AtomicBool::new(false));
    let cancel_app = http::router_with_storage_and_limit(
        RouteOptimizationService::new(
            DevelopmentRoutingProvider,
            SlowCancellationSolver {
                started: started.clone(),
                finished: Arc::new(AtomicBool::new(false)),
            },
        ),
        None,
        Some(cancel_store.clone()),
        None,
    );
    let submitted = send_to(
        cancel_app.clone(),
        Method::POST,
        "/integration/jobs",
        Some("application/json"),
        request_with_job_id("async-cancel").to_string(),
    )
    .await;
    assert_eq!(submitted.status, StatusCode::ACCEPTED);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !started.load(Ordering::Acquire) {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    let cancelled = send_to(
        cancel_app,
        Method::POST,
        "/integration/jobs/async-cancel/cancel",
        None,
        String::new(),
    )
    .await;
    assert_eq!(cancelled.status, StatusCode::OK);
    assert_eq!(cancelled.body["status"], "cancelled");
    wait_for_job_status(cancel_store.as_ref(), "async-cancel", JobStatus::Cancelled).await;
}

#[tokio::test]
async fn persistent_job_api_restores_completed_jobs_and_rejects_duplicates() {
    let temporary = TestDirectory::new();
    let app = app_with_file_storage(&temporary.0);
    let response = send_to(
        app.clone(),
        Method::POST,
        "/optimize",
        Some("application/json"),
        valid_request().to_string(),
    )
    .await;
    assert_eq!(response.status, StatusCode::OK);

    let duplicate = send_to(
        app.clone(),
        Method::POST,
        "/optimize",
        Some("application/json"),
        valid_request().to_string(),
    )
    .await;
    assert_error(duplicate, StatusCode::CONFLICT, "DUPLICATE_JOB_ID");

    let list = send_to(
        app.clone(),
        Method::GET,
        "/integration/jobs?limit=50",
        None,
        String::new(),
    )
    .await;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.body["jobs"].as_array().unwrap().len(), 1);
    assert_eq!(list.body["jobs"][0]["status"], "completed");

    let detail = send_to(
        app.clone(),
        Method::GET,
        "/integration/jobs/route-http-integration-test",
        None,
        String::new(),
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(detail.body["request"], valid_request());
    assert_eq!(detail.body["job_id"], "route-http-integration-test");
    assert_eq!(detail.body["status"], "completed");
    assert_eq!(detail.body["progress"], 100);
    assert_eq!(detail.body["result"], response.body);
    assert!(detail.body["error"].is_null());

    let timeline = send_to(
        app,
        Method::GET,
        "/integration/jobs/route-http-integration-test/timeline",
        None,
        String::new(),
    )
    .await;
    assert_eq!(timeline.status, StatusCode::OK);
    let timeline_entries = timeline.body["entries"].as_array().unwrap();
    assert_eq!(timeline_entries.len(), 4);
    assert_eq!(
        timeline_entries[2]["path"],
        "/integration/jobs/route-http-integration-test"
    );
    assert_eq!(timeline_entries[2]["method"], "GET");
    assert_eq!(timeline_entries[3]["status"], 200);
    assert!(temporary
        .0
        .join("jobs/route-http-integration-test/request.json")
        .is_file());
    assert!(temporary
        .0
        .join("jobs/route-http-integration-test/state.json")
        .is_file());
    assert!(temporary
        .0
        .join("jobs/route-http-integration-test/result.json")
        .is_file());
    assert!(temporary
        .0
        .join("jobs/route-http-integration-test/timeline.jsonl")
        .is_file());
}

#[tokio::test]
async fn persistent_job_failure_writes_error_and_terminal_state() {
    let temporary = TestDirectory::new();
    let app = app_with_file_storage(&temporary.0);
    let mut request = valid_request();
    request["job_id"] = json!("route-persistent-failure");
    request["locations"][1]["close_time"] = json!("09:10");

    let response = send_to(
        app.clone(),
        Method::POST,
        "/optimize",
        Some("application/json"),
        request.to_string(),
    )
    .await;
    assert_error(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        "NO_FEASIBLE_ROUTE",
    );

    let detail = send_to(
        app,
        Method::GET,
        "/integration/jobs/route-persistent-failure",
        None,
        String::new(),
    )
    .await;
    assert_eq!(detail.status, StatusCode::OK);
    assert_eq!(detail.body["status"], "failed");
    assert!(detail.body["completed_at"].is_number());
    assert_eq!(detail.body["error"]["code"], "NO_FEASIBLE_ROUTE");
    assert!(temporary
        .0
        .join("jobs/route-persistent-failure/error.json")
        .is_file());
    assert!(!temporary
        .0
        .join("jobs/route-persistent-failure/result.json")
        .exists());
}

#[tokio::test]
async fn cancel_endpoint_handles_pending_terminal_duplicate_and_unknown_jobs() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let observation =
        JobObservationRecorder::new(Arc::new(FileJobTimelineStore::new(&temporary.0).unwrap()));
    let app = http::router_with_storage(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        Some(observation),
        Some(store.clone()),
    );

    let mut pending_request = valid_request();
    pending_request["job_id"] = json!("pending-cancel");
    store
        .create_job(&serde_json::from_value(pending_request).unwrap())
        .unwrap();
    let cancelled = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs/pending-cancel/cancel",
        None,
        String::new(),
    )
    .await;
    assert_eq!(cancelled.status, StatusCode::OK);
    assert_eq!(
        cancelled.body,
        json!({"job_id":"pending-cancel","status":"cancelled"})
    );
    let stored = store.get_job("pending-cancel").unwrap().unwrap();
    assert_eq!(stored.state.status, JobStatus::Cancelled);
    assert!(stored.state.completed_at.is_some());
    assert!(stored.result.is_none());
    assert!(stored.error.is_none());

    let duplicate = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs/pending-cancel/cancel",
        None,
        String::new(),
    )
    .await;
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
    assert_eq!(duplicate.body["error"]["code"], "JOB_NOT_CANCELLABLE");
    assert_eq!(duplicate.body["error"]["detail"], "cancelled");

    let mut completed_request = valid_request();
    completed_request["job_id"] = json!("completed-cancel");
    assert_eq!(
        send_to(
            app.clone(),
            Method::POST,
            "/optimize",
            Some("application/json"),
            completed_request.to_string(),
        )
        .await
        .status,
        StatusCode::OK
    );
    let completed = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs/completed-cancel/cancel",
        None,
        String::new(),
    )
    .await;
    assert_eq!(completed.status, StatusCode::CONFLICT);
    assert_eq!(completed.body["error"]["detail"], "completed");

    let mut failed_request = valid_request();
    failed_request["job_id"] = json!("failed-cancel");
    failed_request["locations"][1]["close_time"] = json!("09:10");
    assert_eq!(
        send_to(
            app.clone(),
            Method::POST,
            "/optimize",
            Some("application/json"),
            failed_request.to_string(),
        )
        .await
        .status,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let failed = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs/failed-cancel/cancel",
        None,
        String::new(),
    )
    .await;
    assert_eq!(failed.status, StatusCode::CONFLICT);
    assert_eq!(failed.body["error"]["detail"], "failed");

    let unknown = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs/unknown/cancel",
        None,
        String::new(),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.body["error"]["code"], "JOB_NOT_FOUND");

    let timeline = send_to(
        app,
        Method::GET,
        "/integration/jobs/pending-cancel/timeline",
        None,
        String::new(),
    )
    .await;
    let cancel_entries = timeline.body["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| {
            entry["path"] == "/integration/jobs/pending-cancel/cancel"
                || entry["pair_id"]
                    .as_str()
                    .is_some_and(|pair_id| pair_id.starts_with("cancel-"))
        })
        .collect::<Vec<_>>();
    assert_eq!(cancel_entries.len(), 4);
    assert_eq!(cancel_entries[0]["direction"], "REQUEST");
    assert_eq!(cancel_entries[1]["status"], 200);
    assert_eq!(cancel_entries[2]["direction"], "REQUEST");
    assert_eq!(cancel_entries[3]["status"], 409);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_cancel_stops_pipeline_and_persists_cancelled_state() {
    let temporary = TestDirectory::new();
    let started = Arc::new(AtomicBool::new(false));
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let observation =
        JobObservationRecorder::new(Arc::new(FileJobTimelineStore::new(&temporary.0).unwrap()));
    let app = http::router_with_storage(
        RouteOptimizationService::new(
            DevelopmentRoutingProvider,
            SlowCancellationSolver {
                started: started.clone(),
                finished: Arc::new(AtomicBool::new(false)),
            },
        ),
        Some(observation),
        Some(store.clone()),
    );
    let mut request = valid_request();
    request["job_id"] = json!("running-cancel");
    let optimize_app = app.clone();
    let optimize = tokio::spawn(async move {
        send_to(
            optimize_app,
            Method::POST,
            "/optimize",
            Some("application/json"),
            request.to_string(),
        )
        .await
    });

    tokio::time::timeout(Duration::from_secs(2), async {
        while !started.load(Ordering::Acquire) {
            sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();

    let cancelled = send_to(
        app.clone(),
        Method::POST,
        "/integration/jobs/running-cancel/cancel",
        None,
        String::new(),
    )
    .await;
    assert_eq!(cancelled.status, StatusCode::OK);
    assert_eq!(cancelled.body["status"], "cancelled");

    let optimize = optimize.await.unwrap();
    assert_error(optimize, StatusCode::CONFLICT, "JOB_CANCELLED");
    let job = store.get_job("running-cancel").unwrap().unwrap();
    assert_eq!(job.state.status, JobStatus::Cancelled);
    assert!(job.state.completed_at.is_some());
    assert!(job.result.is_none());
    assert!(job.error.is_none());

    let timeline = send_to(
        app,
        Method::GET,
        "/integration/jobs/running-cancel/timeline",
        None,
        String::new(),
    )
    .await;
    let entries = timeline.body["entries"].as_array().unwrap();
    assert!(entries.iter().any(|entry| {
        entry["direction"] == "REQUEST"
            && entry["path"] == "/integration/jobs/running-cancel/cancel"
    }));
    assert!(entries.iter().any(|entry| {
        entry["direction"] == "RESPONSE"
            && entry["status"] == 200
            && entry["source"] == "troute"
            && entry["target"] == "testbed"
    }));
    assert!(entries.iter().all(|entry| {
        matches!(entry["source"].as_str(), Some("testbed" | "troute"))
            && matches!(entry["target"].as_str(), Some("testbed" | "troute"))
    }));
}

#[tokio::test]
async fn optimize_traffic_is_paired_and_available_from_the_timeline_endpoint() {
    let (app, observation) = app_with_observation();
    let body = valid_request();
    let request = Request::builder()
        .method(Method::POST)
        .uri("/optimize")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, "Bearer must-not-be-recorded")
        .header(header::COOKIE, "session=must-not-be-recorded")
        .header("x-api-key", "must-not-be-recorded")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = send_request_to(app.clone(), request).await;

    assert_eq!(response.status, StatusCode::OK);
    let entries = observation.list("route-http-integration-test");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].pair_id, entries[1].pair_id);
    assert_ne!(entries[0].id, entries[1].id);
    assert_eq!(entries[0].direction, ObservationDirection::Request);
    assert_eq!(entries[0].source, ObservationPeer::Testbed);
    assert_eq!(entries[0].target, ObservationPeer::Troute);
    assert_eq!(entries[0].method.as_deref(), Some("POST"));
    assert_eq!(entries[0].path.as_deref(), Some("/optimize"));
    assert_eq!(entries[0].body.as_ref().unwrap(), &body);
    let headers = entries[0].headers.as_ref().unwrap();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers["content-type"], "application/json");
    assert!(!headers.contains_key("authorization"));
    assert!(!headers.contains_key("cookie"));
    assert!(!headers.contains_key("x-api-key"));
    assert_eq!(entries[1].direction, ObservationDirection::Response);
    assert_eq!(entries[1].source, ObservationPeer::Troute);
    assert_eq!(entries[1].target, ObservationPeer::Testbed);
    assert_eq!(entries[1].status, Some(200));
    assert_eq!(entries[1].body.as_ref().unwrap(), &response.body);
    assert!(entries[1].latency_ms.unwrap() >= 0.0);

    let timeline = send_to(
        app.clone(),
        Method::GET,
        "/integration/jobs/route-http-integration-test/timeline",
        None,
        String::new(),
    )
    .await;
    assert_eq!(timeline.status, StatusCode::OK);
    assert_eq!(timeline.body["job_id"], "route-http-integration-test");
    assert_eq!(timeline.body["entries"].as_array().unwrap().len(), 2);
    assert_eq!(timeline.body["entries"][0]["direction"], "REQUEST");
    assert_eq!(timeline.body["entries"][1]["direction"], "RESPONSE");

    let unknown = send_to(
        app,
        Method::GET,
        "/integration/jobs/unknown/timeline",
        None,
        String::new(),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::OK);
    assert_eq!(unknown.body, json!({ "job_id": "unknown", "entries": [] }));
}

#[tokio::test]
async fn timeline_endpoint_sorts_by_timestamp_and_preserves_equal_time_order() {
    let (app, observation) = app_with_observation();
    let mut latest = observed_entry(&observation, "latest");
    let mut first_equal = observed_entry(&observation, "first-equal");
    let mut second_equal = observed_entry(&observation, "second-equal");
    latest.timestamp_ms = 20;
    first_equal.timestamp_ms = 10;
    second_equal.timestamp_ms = 10;
    observation.append("ordered-job", latest);
    observation.append("ordered-job", first_equal);
    observation.append("ordered-job", second_equal);

    let response = send_to(
        app,
        Method::GET,
        "/integration/jobs/ordered-job/timeline",
        None,
        String::new(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["pair_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["first-equal", "second-equal", "latest"]
    );
}

struct PanickingTimelineStore;

impl JobTimelineStore for PanickingTimelineStore {
    fn append(&self, _job_id: &str, _entry: JobTimelineEntry) {
        panic!("simulated observation failure");
    }

    fn list(&self, _job_id: &str) -> Vec<JobTimelineEntry> {
        panic!("simulated observation failure");
    }

    fn clear(&self, _job_id: &str) {
        panic!("simulated observation failure");
    }
}

#[tokio::test]
async fn observation_failure_does_not_affect_optimize() {
    let observation = JobObservationRecorder::new(Arc::new(PanickingTimelineStore));
    let app = http::router_with_observation(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        Some(observation),
    );

    let response = send_to(
        app,
        Method::POST,
        "/optimize",
        Some("application/json"),
        valid_request().to_string(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body["total_travel_minutes"], 30);
}

fn observed_entry(observation: &JobObservationRecorder, pair_id: &str) -> JobTimelineEntry {
    observation.entry(
        pair_id,
        ObservationDirection::Request,
        ObservationPeer::Testbed,
        ObservationPeer::Troute,
    )
}

#[tokio::test]
async fn removed_trasolve_health_endpoint_returns_not_found() {
    let response = send(
        Method::GET,
        "/integration/trasolve/health",
        None,
        String::new(),
    )
    .await;

    assert_error(response, StatusCode::NOT_FOUND, "NOT_FOUND");
}

#[tokio::test]
async fn valid_optimize_request_uses_the_service_pipeline() {
    let response = send(
        Method::POST,
        "/optimize",
        Some("application/json"),
        valid_request().to_string(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers[header::CONTENT_TYPE], "application/json");
    assert_eq!(response.body["total_travel_minutes"], 30);
    assert_eq!(response.body["route"][0]["location_id"], "place-1");
    assert_eq!(response.body["route"][0]["arrival_time"], "09:00");
    assert_eq!(response.body["route"][1]["location_id"], "place-2");
    assert_eq!(response.body["route"][1]["arrival_time"], "09:15");
    assert_eq!(response.body["route"][1]["departure_time"], "09:45");
    assert_eq!(response.body["route"][2]["location_id"], "place-3");
    assert_eq!(response.body["route"][2]["arrival_time"], "10:00");
    assert_eq!(response.body["route"][2]["departure_time"], "10:00");
}

#[tokio::test]
async fn two_locations_form_a_direct_start_to_destination_route() {
    let mut request = valid_request();
    let locations = request["locations"].as_array().unwrap();
    let start = locations.first().unwrap().clone();
    let destination = locations.last().unwrap().clone();
    request["locations"] = json!([start, destination]);

    let response = send(
        Method::POST,
        "/optimize",
        Some("application/json"),
        request.to_string(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body["route"].as_array().unwrap().len(), 2);
    assert_eq!(response.body["route"][0]["location_id"], "place-1");
    assert_eq!(response.body["route"][1]["location_id"], "place-3");
}

#[tokio::test]
async fn job_id_at_the_maximum_length_is_accepted() {
    let mut request = valid_request();
    request["job_id"] = json!("j".repeat(128));
    let response = send(
        Method::POST,
        "/optimize",
        Some("application/json"),
        request.to_string(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
}

#[tokio::test]
async fn malformed_json_returns_a_json_client_error() {
    let response = send(
        Method::POST,
        "/optimize",
        Some("application/json"),
        "{".to_owned(),
    )
    .await;
    assert_error(response, StatusCode::BAD_REQUEST, "INVALID_REQUEST");
}

#[tokio::test]
async fn invalid_requests_return_stable_json_errors_without_panicking() {
    let mut empty_job_id = valid_request();
    empty_job_id["job_id"] = json!("");
    let mut whitespace_job_id = valid_request();
    whitespace_job_id["job_id"] = json!("   ");
    let mut long_job_id = valid_request();
    long_job_id["job_id"] = json!("j".repeat(129));
    let mut missing_job_id = valid_request();
    missing_job_id.as_object_mut().unwrap().remove("job_id");
    let mut invalid_time = valid_request();
    invalid_time["start_time"] = json!("9:00");
    let mut empty_locations = valid_request();
    empty_locations["locations"] = json!([]);
    let mut one_location = valid_request();
    one_location["locations"] = json!([one_location["locations"][0].clone()]);
    let mut duplicate_ids = valid_request();
    duplicate_ids["locations"][1]["id"] = json!("place-1");
    let mut invalid_window = valid_request();
    invalid_window["locations"][0]["open_time"] = json!("19:00");
    invalid_window["locations"][0]["close_time"] = json!("18:00");
    let mut incorrect_type = valid_request();
    incorrect_type["locations"][0]["stay_minutes"] = json!("sixty");
    let mut too_many_locations = valid_request();
    too_many_locations["locations"] = Value::Array(
        (0..501)
            .map(|index| {
                json!({
                    "id": format!("place-{index}"),
                    "place_id": format!("google-place-{index}"),
                    "open_time": "09:00",
                    "close_time": "18:00",
                    "stay_minutes": 0
                })
            })
            .collect(),
    );
    let mut location_id_too_long = valid_request();
    location_id_too_long["locations"][0]["id"] = json!("x".repeat(513));
    let mut unknown_field = valid_request();
    unknown_field["unexpected"] = json!(true);

    for body in [
        empty_job_id,
        whitespace_job_id,
        long_job_id,
        missing_job_id,
        invalid_time,
        empty_locations,
        one_location,
        duplicate_ids,
        invalid_window,
        incorrect_type,
        too_many_locations,
        location_id_too_long,
        unknown_field,
    ] {
        let response = send(
            Method::POST,
            "/optimize",
            Some("application/json"),
            body.to_string(),
        )
        .await;
        assert_error(response, StatusCode::BAD_REQUEST, "INVALID_REQUEST");
    }
}

#[tokio::test]
async fn infeasible_schedule_has_the_documented_error() {
    let mut request = valid_request();
    request["locations"][1]["close_time"] = json!("09:10");
    let response = send(
        Method::POST,
        "/optimize",
        Some("application/json"),
        request.to_string(),
    )
    .await;
    assert_error(
        response,
        StatusCode::UNPROCESSABLE_ENTITY,
        "NO_FEASIBLE_ROUTE",
    );
}

#[tokio::test]
async fn content_type_method_and_body_limit_errors_are_json() {
    let unsupported = send(
        Method::POST,
        "/optimize",
        Some("text/plain"),
        valid_request().to_string(),
    )
    .await;
    assert_error(
        unsupported,
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "UNSUPPORTED_MEDIA_TYPE",
    );

    let method = send(Method::GET, "/optimize", None, String::new()).await;
    assert_eq!(method.headers[header::ALLOW], "POST");
    assert_error(method, StatusCode::METHOD_NOT_ALLOWED, "METHOD_NOT_ALLOWED");

    let oversized = send(
        Method::POST,
        "/optimize",
        Some("application/json"),
        format!(r#"{{"padding":"{}"}}"#, "x".repeat(1024 * 1024)),
    )
    .await;
    assert_error(
        oversized,
        StatusCode::PAYLOAD_TOO_LARGE,
        "REQUEST_TOO_LARGE",
    );
}

fn assert_error(response: Response, status: StatusCode, code: &str) {
    assert_eq!(response.status, status);
    assert_eq!(response.headers[header::CONTENT_TYPE], "application/json");
    assert_eq!(response.body["error"]["code"], code);
    assert!(response.body["error"]["message"].is_string());
}
