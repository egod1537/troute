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
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
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

#[tokio::test]
async fn health_remains_available() {
    let response = send(Method::GET, "/health", None, String::new()).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body, json!({ "status": "ok" }));
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
