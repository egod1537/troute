use axum::{
    body::Body,
    http::{header, Method, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use troute::{
    development::{DevelopmentRouteSolver, DevelopmentRoutingProvider},
    http, RouteOptimizationService,
};

fn app() -> Router {
    http::router(RouteOptimizationService::new(
        DevelopmentRoutingProvider,
        DevelopmentRouteSolver,
    ))
}

fn valid_request() -> Value {
    json!({
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
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    let response = app()
        .oneshot(builder.body(Body::from(body)).unwrap())
        .await
        .unwrap();
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
