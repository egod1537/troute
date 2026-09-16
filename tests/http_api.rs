use axum::{
    body::Body,
    extract::State,
    http::{header, Method, Request, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle, time::sleep};
use tower::ServiceExt;
use troute::{
    development::{DevelopmentRouteSolver, DevelopmentRoutingProvider},
    http,
    observation::{
        JobObservationRecorder, JobTimelineEntry, JobTimelineStore, ObservationDirection,
        ObservationPeer,
    },
    trasolve::TrasolveClient,
    RouteOptimizationService,
};

fn app() -> Router {
    http::router(RouteOptimizationService::new(
        DevelopmentRoutingProvider,
        DevelopmentRouteSolver,
    ))
}

fn app_with_trasolve(trasolve: TrasolveClient) -> Router {
    http::router_with_trasolve(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        Some(trasolve),
    )
}

fn app_with_observation(trasolve: Option<TrasolveClient>) -> (Router, JobObservationRecorder) {
    let observation = JobObservationRecorder::in_memory();
    let app = http::router_with_observation(
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver),
        trasolve,
        Some(observation.clone()),
    );
    (app, observation)
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
            }
        ],
        "start_location_id": "place-1",
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
async fn optimize_traffic_is_paired_and_available_from_the_timeline_endpoint() {
    let (app, observation) = app_with_observation(None);
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
async fn optimize_timeline_contains_real_callback_request_response_pairs() {
    let (base_url, mut callback_events, _server) =
        spawn_job_event_mock(StatusCode::NO_CONTENT).await;
    let (app, observation) = app_with_observation(Some(TrasolveClient::new(base_url).unwrap()));

    let response = send_to(
        app,
        Method::POST,
        "/optimize",
        Some("application/json"),
        valid_request().to_string(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    let mut delivered = 0;
    while callback_events.try_recv().is_ok() {
        delivered += 1;
    }
    assert_eq!(delivered, 5);

    let entries = observation.list("route-http-integration-test");
    assert_eq!(entries.len(), 12);
    let callback_requests = entries
        .iter()
        .filter(|entry| {
            entry.direction == ObservationDirection::Request
                && entry.source == ObservationPeer::Troute
                && entry.target == ObservationPeer::Trasolve
        })
        .collect::<Vec<_>>();
    let callback_responses = entries
        .iter()
        .filter(|entry| {
            entry.direction == ObservationDirection::Response
                && entry.source == ObservationPeer::Trasolve
                && entry.target == ObservationPeer::Troute
        })
        .collect::<Vec<_>>();
    assert_eq!(callback_requests.len(), 5);
    assert_eq!(callback_responses.len(), 5);
    for request in callback_requests {
        let response = callback_responses
            .iter()
            .find(|response| response.pair_id == request.pair_id)
            .unwrap();
        assert_eq!(response.status, Some(204));
    }
}

#[tokio::test]
async fn timeline_endpoint_sorts_by_timestamp_and_preserves_equal_time_order() {
    let (app, observation) = app_with_observation(None);
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
        None,
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
async fn trasolve_integration_health_succeeds_with_a_local_mock() {
    let (base_url, _server) = spawn_trasolve_mock(
        StatusCode::OK,
        r#"{"status":"ok","service":"trasolve"}"#,
        Duration::ZERO,
    )
    .await;
    let response = send_to(
        app_with_trasolve(TrasolveClient::new(base_url).unwrap()),
        Method::GET,
        "/integration/trasolve/health",
        None,
        String::new(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.body,
        json!({
            "status": "ok",
            "trasolve": { "status": "ok", "service": "trasolve" }
        })
    );
}

#[tokio::test]
async fn trasolve_integration_health_is_deterministic_when_not_configured() {
    let response = send(
        Method::GET,
        "/integration/trasolve/health",
        None,
        String::new(),
    )
    .await;

    assert_error(
        response,
        StatusCode::SERVICE_UNAVAILABLE,
        "TRASOLVE_NOT_CONFIGURED",
    );
}

#[tokio::test]
async fn trasolve_integration_timeout_maps_to_gateway_timeout() {
    let (base_url, _server) = spawn_trasolve_mock(
        StatusCode::OK,
        r#"{"status":"ok","service":"trasolve"}"#,
        Duration::from_millis(100),
    )
    .await;
    let client = TrasolveClient::with_timeout(base_url, Duration::from_millis(10)).unwrap();
    let response = send_to(
        app_with_trasolve(client),
        Method::GET,
        "/integration/trasolve/health",
        None,
        String::new(),
    )
    .await;

    assert_error(response, StatusCode::GATEWAY_TIMEOUT, "TRASOLVE_TIMEOUT");
}

#[tokio::test]
async fn malformed_trasolve_response_maps_to_bad_gateway() {
    let (base_url, _server) = spawn_trasolve_mock(StatusCode::OK, "not-json", Duration::ZERO).await;
    let response = send_to(
        app_with_trasolve(TrasolveClient::new(base_url).unwrap()),
        Method::GET,
        "/integration/trasolve/health",
        None,
        String::new(),
    )
    .await;

    assert_error(
        response,
        StatusCode::BAD_GATEWAY,
        "TRASOLVE_INVALID_RESPONSE",
    );
}

#[tokio::test]
async fn callback_connection_failure_does_not_alter_optimize_result() {
    let client = TrasolveClient::new("http://127.0.0.1:1").unwrap();
    let response = send_to(
        app_with_trasolve(client),
        Method::POST,
        "/optimize",
        Some("application/json"),
        valid_request().to_string(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body["total_travel_minutes"], 30);
}

#[tokio::test]
async fn callback_rejections_do_not_change_successful_optimize_response() {
    let (base_url, mut events, _server) =
        spawn_job_event_mock(StatusCode::SERVICE_UNAVAILABLE).await;
    let response = send_to(
        app_with_trasolve(TrasolveClient::new(base_url).unwrap()),
        Method::POST,
        "/optimize",
        Some("application/json"),
        valid_request().to_string(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.body["total_travel_minutes"], 30);
    let mut callbacks = Vec::new();
    while let Ok(event) = events.try_recv() {
        callbacks.push(event);
    }
    assert_eq!(callbacks.len(), 5);
    assert_eq!(callbacks.last().unwrap()["type"], "result");
}

#[tokio::test]
async fn optimize_emits_progress_then_one_matching_result_callback() {
    let (base_url, mut events, _server) = spawn_job_event_mock(StatusCode::NO_CONTENT).await;
    let response = send_to(
        app_with_trasolve(TrasolveClient::new(base_url).unwrap()),
        Method::POST,
        "/optimize",
        Some("application/json"),
        valid_request().to_string(),
    )
    .await;

    assert_eq!(response.status, StatusCode::OK);
    let mut callbacks = Vec::new();
    while let Ok(event) = events.try_recv() {
        callbacks.push(event);
    }
    assert_eq!(callbacks.len(), 5);
    assert_eq!(
        callbacks
            .iter()
            .map(|event| event["sequence"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5]
    );
    let progress_events = &callbacks[..4];
    assert!(progress_events
        .iter()
        .all(|event| event["type"] == "progress"));
    assert_eq!(
        progress_events
            .iter()
            .map(|event| event["data"]["stage"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["accepted", "building_matrix", "solving", "scheduling"]
    );
    let progress = progress_events
        .iter()
        .map(|event| event["data"]["progress"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(progress, vec![0, 20, 60, 85]);
    assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(!progress.contains(&100));

    let result_events = callbacks
        .iter()
        .filter(|event| event["type"] == "result")
        .collect::<Vec<_>>();
    assert_eq!(result_events.len(), 1);
    assert_eq!(result_events[0]["sequence"], 5);
    assert_eq!(result_events[0]["data"], response.body);
}

#[tokio::test]
async fn infeasible_schedule_emits_a_structured_error_callback() {
    let (base_url, mut events, _server) = spawn_job_event_mock(StatusCode::NO_CONTENT).await;
    let mut request = valid_request();
    request["locations"][1]["close_time"] = json!("09:10");
    let response = send_to(
        app_with_trasolve(TrasolveClient::new(base_url).unwrap()),
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
    let mut callbacks = Vec::new();
    while let Ok(event) = events.try_recv() {
        callbacks.push(event);
    }
    let error = callbacks.last().unwrap();
    assert_eq!(error["sequence"], 5);
    assert_eq!(error["type"], "error");
    assert_eq!(error["data"]["code"], "NO_FEASIBLE_ROUTE");
    assert_eq!(error["data"]["message"], "No feasible route was found.");
    assert!(error["data"]["detail"]
        .as_str()
        .unwrap()
        .contains("time window"));
    assert!(!callbacks.iter().any(|event| event["type"] == "result"));
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
    assert_eq!(response.body["route"][2]["location_id"], "place-1");
    assert!(response.body["route"][2].get("departure_time").is_none());
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
    let mut duplicate_ids = valid_request();
    duplicate_ids["locations"][1]["id"] = json!("place-1");
    let mut unknown_start = valid_request();
    unknown_start["start_location_id"] = json!("missing");
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
    too_many_locations["start_location_id"] = json!("place-0");
    let mut location_id_too_long = valid_request();
    location_id_too_long["locations"][0]["id"] = json!("x".repeat(513));
    location_id_too_long["start_location_id"] = json!("x".repeat(513));
    let mut unknown_field = valid_request();
    unknown_field["unexpected"] = json!(true);

    for body in [
        empty_job_id,
        whitespace_job_id,
        long_job_id,
        missing_job_id,
        invalid_time,
        empty_locations,
        duplicate_ids,
        unknown_start,
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

#[derive(Clone)]
struct MockTrasolveResponse {
    status: StatusCode,
    body: &'static str,
    delay: Duration,
}

async fn spawn_trasolve_mock(
    status: StatusCode,
    body: &'static str,
    delay: Duration,
) -> (String, JoinHandle<()>) {
    async fn respond(State(response): State<MockTrasolveResponse>) -> impl IntoResponse {
        sleep(response.delay).await;
        (response.status, response.body)
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/api/internal/troute/health", get(respond))
        .with_state(MockTrasolveResponse {
            status,
            body,
            delay,
        });
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), server)
}

async fn spawn_job_event_mock(
    status: StatusCode,
) -> (String, mpsc::UnboundedReceiver<Value>, JoinHandle<()>) {
    #[derive(Clone)]
    struct CallbackState {
        status: StatusCode,
        events: mpsc::UnboundedSender<Value>,
    }

    async fn receive_event(
        State(state): State<CallbackState>,
        Json(event): Json<Value>,
    ) -> impl IntoResponse {
        state.events.send(event).unwrap();
        state.status
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let app = Router::new()
        .route("/api/internal/troute/jobs/{*path}", post(receive_event))
        .with_state(CallbackState {
            status,
            events: event_tx,
        });
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), event_rx, server)
}
