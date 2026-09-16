use std::{
    fmt,
    time::{Duration, Instant},
};

use reqwest::{redirect::Policy, StatusCode, Url};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    events::JobEvent,
    observation::{
        allowlisted_headers, json_or_raw, JobObservationRecorder, ObservationDirection,
        ObservationPeer,
    },
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_PATH: &str = "api/internal/troute/health";
const JOBS_PATH: &str = "api/internal/troute/jobs";

/// Reusable asynchronous client for troute-to-Trasolve communication.
#[derive(Clone)]
pub struct TrasolveClient {
    base_url: String,
    health_url: Url,
    jobs_url: Url,
    http: reqwest::Client,
    observation: Option<JobObservationRecorder>,
}

impl fmt::Debug for TrasolveClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrasolveClient")
            .field("base_url", &self.base_url)
            .field("health_url", &self.health_url)
            .field("jobs_url", &self.jobs_url)
            .field("observation_enabled", &self.observation.is_some())
            .finish_non_exhaustive()
    }
}

impl TrasolveClient {
    /// Creates a client with the default five-second request timeout.
    pub fn new(base_url: impl AsRef<str>) -> Result<Self, TrasolveClientError> {
        Self::with_timeout(base_url, DEFAULT_TIMEOUT)
    }

    /// Creates a client with an explicit total request timeout.
    pub fn with_timeout(
        base_url: impl AsRef<str>,
        timeout: Duration,
    ) -> Result<Self, TrasolveClientError> {
        if timeout.is_zero() {
            return Err(TrasolveClientError::Configuration(
                "Trasolve request timeout must be greater than zero".to_owned(),
            ));
        }

        let base_url = normalize_base_url(base_url.as_ref())?;
        let health_url = Url::parse(&format!("{base_url}/{HEALTH_PATH}")).map_err(|error| {
            TrasolveClientError::Configuration(format!(
                "TRASOLVE_BASE_URL cannot be used as a request base URL: {error}"
            ))
        })?;
        let jobs_url = Url::parse(&format!("{base_url}/{JOBS_PATH}")).map_err(|error| {
            TrasolveClientError::Configuration(format!(
                "TRASOLVE_BASE_URL cannot be used as a job callback base URL: {error}"
            ))
        })?;
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(Policy::none())
            .build()
            .map_err(|error| {
                TrasolveClientError::Configuration(format!(
                    "failed to construct Trasolve HTTP client: {error}"
                ))
            })?;

        Ok(Self {
            base_url,
            health_url,
            jobs_url,
            http,
            observation: None,
        })
    }

    /// Enables best-effort job callback observation for this client.
    pub fn with_observation(mut self, observation: JobObservationRecorder) -> Self {
        self.observation = Some(observation);
        self
    }

    /// Returns the normalized configured base URL without trailing slashes.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Checks the internal Trasolve endpoint and validates its response contract.
    pub async fn health(&self) -> Result<TrasolveHealthResponse, TrasolveClientError> {
        let response = self
            .http
            .get(self.health_url.clone())
            .send()
            .await
            .map_err(classify_request_error)?;

        if response.status() != StatusCode::OK {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .map_err(classify_request_error)
                .map(|body| (!body.is_empty()).then_some(body))?;
            return Err(TrasolveClientError::UpstreamHttp { status, body });
        }

        let health = response
            .json::<TrasolveHealthResponse>()
            .await
            .map_err(classify_response_error)?;
        if health.status != "ok" {
            return Err(TrasolveClientError::InvalidResponse(format!(
                "expected status `ok`, received `{}`",
                health.status
            )));
        }
        if health.service != "trasolve" {
            return Err(TrasolveClientError::InvalidResponse(format!(
                "expected service `trasolve`, received `{}`",
                health.service
            )));
        }

        Ok(health)
    }

    /// Sends one job-correlated event to Trasolve.
    pub async fn send_job_event<T: Serialize>(
        &self,
        job_id: &str,
        event: &JobEvent<T>,
    ) -> Result<(), TrasolveClientError> {
        let mut url = self.jobs_url.clone();
        url.path_segments_mut()
            .map_err(|_| {
                TrasolveClientError::Configuration(
                    "TRASOLVE_BASE_URL cannot be used for job callbacks".to_owned(),
                )
            })?
            .push(job_id)
            .push("events");

        let request = self
            .http
            .post(url)
            .json(event)
            .build()
            .map_err(classify_request_error)?;
        let path = request.url().path().to_owned();
        let pair_id = self
            .observation
            .as_ref()
            .map(|observation| observation.next_pair_id("cb"));

        if let (Some(observation), Some(pair_id)) = (&self.observation, &pair_id) {
            let mut entry = observation.entry(
                pair_id,
                ObservationDirection::Request,
                ObservationPeer::Troute,
                ObservationPeer::Trasolve,
            );
            entry.method = Some("POST".to_owned());
            entry.path = Some(path);
            entry.headers = allowlisted_headers(request.headers());
            entry.body = serde_json::to_value(event).ok();
            observation.append(job_id, entry);
        }

        let started = Instant::now();
        let response = match self.http.execute(request).await {
            Ok(response) => response,
            Err(error) => {
                let error = classify_request_error(error);
                self.record_job_event_response(
                    job_id,
                    pair_id.as_deref(),
                    started,
                    None,
                    None,
                    None,
                    None,
                    Some(error.to_string()),
                );
                return Err(error);
            }
        };

        let status = response.status();
        let response_headers = allowlisted_headers(response.headers());
        let response_body = match response.text().await {
            Ok(body) => body,
            Err(error) => {
                let error = classify_request_error(error);
                self.record_job_event_response(
                    job_id,
                    pair_id.as_deref(),
                    started,
                    Some(status.as_u16()),
                    response_headers,
                    None,
                    None,
                    Some(error.to_string()),
                );
                return Err(error);
            }
        };
        let (body, raw) = json_or_raw(response_body.clone());

        if !status.is_success() {
            let error = TrasolveClientError::UpstreamHttp {
                status: status.as_u16(),
                body: (!response_body.is_empty()).then_some(response_body),
            };
            self.record_job_event_response(
                job_id,
                pair_id.as_deref(),
                started,
                Some(status.as_u16()),
                response_headers,
                body,
                raw,
                Some(error.to_string()),
            );
            return Err(error);
        }

        self.record_job_event_response(
            job_id,
            pair_id.as_deref(),
            started,
            Some(status.as_u16()),
            response_headers,
            body,
            raw,
            None,
        );

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn record_job_event_response(
        &self,
        job_id: &str,
        pair_id: Option<&str>,
        started: Instant,
        status: Option<u16>,
        headers: Option<crate::observation::ObservationHeaders>,
        body: Option<serde_json::Value>,
        raw: Option<String>,
        error: Option<String>,
    ) {
        let (Some(observation), Some(pair_id)) = (&self.observation, pair_id) else {
            return;
        };
        let mut entry = observation.entry(
            pair_id,
            ObservationDirection::Response,
            ObservationPeer::Trasolve,
            ObservationPeer::Troute,
        );
        entry.status = status;
        entry.latency_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
        entry.headers = headers;
        entry.body = body;
        entry.raw = raw;
        entry.error = error;
        observation.append(job_id, entry);
    }
}

fn normalize_base_url(value: &str) -> Result<String, TrasolveClientError> {
    if value.is_empty() {
        return Err(TrasolveClientError::Configuration(
            "TRASOLVE_BASE_URL must not be empty".to_owned(),
        ));
    }
    if value.trim() != value {
        return Err(TrasolveClientError::Configuration(
            "TRASOLVE_BASE_URL must not contain surrounding whitespace".to_owned(),
        ));
    }

    let parsed = Url::parse(value).map_err(|error| {
        TrasolveClientError::Configuration(format!("TRASOLVE_BASE_URL is invalid: {error}"))
    })?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(TrasolveClientError::Configuration(
            "TRASOLVE_BASE_URL must use http or https".to_owned(),
        ));
    }
    let authority = value
        .split_once("://")
        .map(|(_, authority)| authority)
        .unwrap_or_default();
    if authority.is_empty() || authority.starts_with('/') {
        return Err(TrasolveClientError::Configuration(
            "TRASOLVE_BASE_URL must include a valid host".to_owned(),
        ));
    }
    if parsed.host_str().is_none() || parsed.cannot_be_a_base() {
        return Err(TrasolveClientError::Configuration(
            "TRASOLVE_BASE_URL must include a valid host".to_owned(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(TrasolveClientError::Configuration(
            "TRASOLVE_BASE_URL must not contain credentials".to_owned(),
        ));
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(TrasolveClientError::Configuration(
            "TRASOLVE_BASE_URL must not contain a query or fragment".to_owned(),
        ));
    }

    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

fn classify_request_error(error: reqwest::Error) -> TrasolveClientError {
    if error.is_timeout() {
        TrasolveClientError::Timeout
    } else {
        TrasolveClientError::Connection(error.to_string())
    }
}

fn classify_response_error(error: reqwest::Error) -> TrasolveClientError {
    if error.is_timeout() {
        TrasolveClientError::Timeout
    } else if error.is_decode() {
        TrasolveClientError::InvalidResponse(error.to_string())
    } else {
        TrasolveClientError::Connection(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrasolveHealthResponse {
    pub status: String,
    pub service: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TrasolveClientError {
    #[error("invalid Trasolve client configuration: {0}")]
    Configuration(String),
    #[error("Trasolve request timed out")]
    Timeout,
    #[error("could not connect to Trasolve: {0}")]
    Connection(String),
    #[error("Trasolve returned HTTP {status}")]
    UpstreamHttp { status: u16, body: Option<String> },
    #[error("Trasolve returned an invalid response: {0}")]
    InvalidResponse(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        events::{JobEvent, JobEventType},
        observation::{JobObservationRecorder, ObservationDirection, ObservationPeer},
    };
    use axum::{
        extract::State,
        http::{HeaderMap, StatusCode, Uri},
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use tokio::{
        net::TcpListener,
        sync::mpsc,
        task::JoinHandle,
        time::{sleep, timeout},
    };

    #[derive(Clone)]
    struct MockResponse {
        status: StatusCode,
        body: &'static str,
        delay: Duration,
    }

    #[derive(Clone)]
    struct CallbackMock {
        status: StatusCode,
        response_body: &'static str,
        delay: Duration,
        requests: mpsc::UnboundedSender<CapturedCallback>,
    }

    #[derive(Debug)]
    struct CapturedCallback {
        uri: String,
        content_type: Option<String>,
        body: Value,
    }

    #[test]
    fn accepts_and_normalizes_valid_base_urls() {
        let client = TrasolveClient::new("http://127.0.0.1:43127").unwrap();
        assert_eq!(client.base_url(), "http://127.0.0.1:43127");

        let client = TrasolveClient::new("https://example.com/base///").unwrap();
        assert_eq!(client.base_url(), "https://example.com/base");
    }

    #[test]
    fn rejects_invalid_base_urls() {
        for value in [
            "",
            "localhost:43127",
            "ftp://example.com",
            "http:///missing-host",
            "http://user:secret@example.com",
            "http://example.com?query=value",
            " http://example.com",
        ] {
            assert!(
                matches!(
                    TrasolveClient::new(value),
                    Err(TrasolveClientError::Configuration(_))
                ),
                "unexpectedly accepted {value}"
            );
        }
    }

    #[tokio::test]
    async fn expected_health_response_is_accepted() {
        let (base_url, _server) = spawn_mock(StatusCode::OK, valid_health(), Duration::ZERO).await;
        let response = TrasolveClient::new(base_url)
            .unwrap()
            .health()
            .await
            .unwrap();

        assert_eq!(response.status, "ok");
        assert_eq!(response.service, "trasolve");
    }

    #[tokio::test]
    async fn wrong_service_and_status_are_rejected() {
        for body in [
            r#"{"status":"ok","service":"another-service"}"#,
            r#"{"status":"degraded","service":"trasolve"}"#,
        ] {
            let (base_url, _server) = spawn_mock(StatusCode::OK, body, Duration::ZERO).await;
            assert!(matches!(
                TrasolveClient::new(base_url).unwrap().health().await,
                Err(TrasolveClientError::InvalidResponse(_))
            ));
        }
    }

    #[tokio::test]
    async fn malformed_json_is_rejected() {
        let (base_url, _server) = spawn_mock(StatusCode::OK, "{", Duration::ZERO).await;
        assert!(matches!(
            TrasolveClient::new(base_url).unwrap().health().await,
            Err(TrasolveClientError::InvalidResponse(_))
        ));
    }

    #[tokio::test]
    async fn timeout_is_classified() {
        let (base_url, _server) =
            spawn_mock(StatusCode::OK, valid_health(), Duration::from_millis(100)).await;
        let client = TrasolveClient::with_timeout(base_url, Duration::from_millis(10)).unwrap();

        assert_eq!(
            client.health().await.unwrap_err(),
            TrasolveClientError::Timeout
        );
    }

    #[tokio::test]
    async fn non_success_status_is_classified_with_its_body() {
        let (base_url, _server) =
            spawn_mock(StatusCode::BAD_GATEWAY, "upstream failed", Duration::ZERO).await;

        assert_eq!(
            TrasolveClient::new(base_url)
                .unwrap()
                .health()
                .await
                .unwrap_err(),
            TrasolveClientError::UpstreamHttp {
                status: 502,
                body: Some("upstream failed".to_owned()),
            }
        );
    }

    #[tokio::test]
    async fn job_event_is_posted_as_json_with_job_id_in_one_encoded_segment() {
        let (base_url, mut requests, _server) =
            spawn_callback_mock(StatusCode::ACCEPTED, "", Duration::ZERO).await;
        let event = JobEvent {
            sequence: 1,
            event_type: JobEventType::Progress,
            data: json!({ "percent": 25 }),
        };

        TrasolveClient::new(base_url)
            .unwrap()
            .send_job_event("route/with spaces?#%", &event)
            .await
            .unwrap();

        let request = timeout(Duration::from_secs(1), requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            request.uri,
            "/api/internal/troute/jobs/route%2Fwith%20spaces%3F%23%25/events"
        );
        assert_eq!(request.content_type.as_deref(), Some("application/json"));
        assert_eq!(
            request.body,
            json!({
                "sequence": 1,
                "type": "progress",
                "data": { "percent": 25 }
            })
        );
    }

    #[tokio::test]
    async fn successful_job_event_records_the_real_request_and_response_pair() {
        let (base_url, _requests, _server) =
            spawn_callback_mock(StatusCode::ACCEPTED, "accepted", Duration::ZERO).await;
        let observation = JobObservationRecorder::in_memory();
        let event = JobEvent {
            sequence: 1,
            event_type: JobEventType::Progress,
            data: json!({ "progress": 20 }),
        };

        TrasolveClient::new(base_url)
            .unwrap()
            .with_observation(observation.clone())
            .send_job_event("route-observed", &event)
            .await
            .unwrap();

        let entries = observation.list("route-observed");
        assert_eq!(entries.len(), 2);
        assert_ne!(entries[0].id, entries[1].id);
        assert_eq!(entries[0].pair_id, entries[1].pair_id);
        assert_eq!(entries[0].direction, ObservationDirection::Request);
        assert_eq!(entries[0].source, ObservationPeer::Troute);
        assert_eq!(entries[0].target, ObservationPeer::Trasolve);
        assert_eq!(entries[0].method.as_deref(), Some("POST"));
        assert_eq!(
            entries[0].path.as_deref(),
            Some("/api/internal/troute/jobs/route-observed/events")
        );
        assert_eq!(entries[0].body.as_ref().unwrap()["type"], "progress");
        assert_eq!(
            entries[0].headers.as_ref().unwrap()["content-type"],
            "application/json"
        );
        assert_eq!(entries[1].direction, ObservationDirection::Response);
        assert_eq!(entries[1].source, ObservationPeer::Trasolve);
        assert_eq!(entries[1].target, ObservationPeer::Troute);
        assert_eq!(entries[1].status, Some(202));
        assert_eq!(entries[1].raw.as_deref(), Some("accepted"));
        assert!(entries[1].latency_ms.unwrap() >= 0.0);
        assert!(entries[1].error.is_none());
    }

    #[tokio::test]
    async fn job_event_non_success_status_is_classified() {
        let (base_url, _requests, _server) =
            spawn_callback_mock(StatusCode::CONFLICT, "sequence rejected", Duration::ZERO).await;
        let event = JobEvent {
            sequence: 1,
            event_type: JobEventType::Error,
            data: json!({}),
        };

        assert_eq!(
            TrasolveClient::new(base_url)
                .unwrap()
                .send_job_event("route-123", &event)
                .await
                .unwrap_err(),
            TrasolveClientError::UpstreamHttp {
                status: 409,
                body: Some("sequence rejected".to_owned()),
            }
        );
    }

    #[tokio::test]
    async fn job_event_http_error_is_recorded_with_status_body_and_error() {
        let (base_url, _requests, _server) =
            spawn_callback_mock(StatusCode::CONFLICT, "sequence rejected", Duration::ZERO).await;
        let observation = JobObservationRecorder::in_memory();
        let event = JobEvent {
            sequence: 1,
            event_type: JobEventType::Error,
            data: json!({}),
        };

        let error = TrasolveClient::new(base_url)
            .unwrap()
            .with_observation(observation.clone())
            .send_job_event("route-rejected", &event)
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            TrasolveClientError::UpstreamHttp { status: 409, .. }
        ));
        let entries = observation.list("route-rejected");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].status, Some(409));
        assert_eq!(entries[1].raw.as_deref(), Some("sequence rejected"));
        assert!(entries[1].error.as_deref().unwrap().contains("HTTP 409"));
    }

    #[tokio::test]
    async fn job_event_timeout_is_classified() {
        let (base_url, _requests, _server) =
            spawn_callback_mock(StatusCode::NO_CONTENT, "", Duration::from_millis(100)).await;
        let client = TrasolveClient::with_timeout(base_url, Duration::from_millis(10)).unwrap();
        let event = JobEvent {
            sequence: 1,
            event_type: JobEventType::Result,
            data: json!({}),
        };

        assert_eq!(
            client
                .send_job_event("route-timeout", &event)
                .await
                .unwrap_err(),
            TrasolveClientError::Timeout
        );
    }

    #[tokio::test]
    async fn job_event_timeout_is_recorded_as_a_response_without_status() {
        let (base_url, _requests, _server) =
            spawn_callback_mock(StatusCode::NO_CONTENT, "", Duration::from_millis(100)).await;
        let observation = JobObservationRecorder::in_memory();
        let client = TrasolveClient::with_timeout(base_url, Duration::from_millis(10))
            .unwrap()
            .with_observation(observation.clone());
        let event = JobEvent {
            sequence: 1,
            event_type: JobEventType::Result,
            data: json!({}),
        };

        assert_eq!(
            client
                .send_job_event("route-timeout-observed", &event)
                .await
                .unwrap_err(),
            TrasolveClientError::Timeout
        );
        let entries = observation.list("route-timeout-observed");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].direction, ObservationDirection::Response);
        assert_eq!(entries[1].status, None);
        assert!(entries[1].latency_ms.unwrap() >= 10.0);
        assert!(entries[1].error.as_deref().unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn job_event_connection_failure_is_recorded_without_status() {
        let observation = JobObservationRecorder::in_memory();
        let client = TrasolveClient::new("http://127.0.0.1:1")
            .unwrap()
            .with_observation(observation.clone());
        let event = JobEvent {
            sequence: 1,
            event_type: JobEventType::Progress,
            data: json!({}),
        };

        assert!(matches!(
            client
                .send_job_event("route-connection-failure", &event)
                .await,
            Err(TrasolveClientError::Connection(_))
        ));
        let entries = observation.list("route-connection-failure");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].direction, ObservationDirection::Response);
        assert_eq!(entries[1].status, None);
        assert!(entries[1]
            .error
            .as_deref()
            .unwrap()
            .contains("could not connect"));
    }

    fn valid_health() -> &'static str {
        r#"{"status":"ok","service":"trasolve"}"#
    }

    async fn spawn_mock(
        status: StatusCode,
        body: &'static str,
        delay: Duration,
    ) -> (String, JoinHandle<()>) {
        async fn respond(State(response): State<MockResponse>) -> impl IntoResponse {
            sleep(response.delay).await;
            (response.status, response.body)
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/api/internal/troute/health", get(respond))
            .with_state(MockResponse {
                status,
                body,
                delay,
            });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), server)
    }

    async fn spawn_callback_mock(
        status: StatusCode,
        response_body: &'static str,
        delay: Duration,
    ) -> (
        String,
        mpsc::UnboundedReceiver<CapturedCallback>,
        JoinHandle<()>,
    ) {
        async fn receive_event(
            State(response): State<CallbackMock>,
            uri: Uri,
            headers: HeaderMap,
            Json(body): Json<Value>,
        ) -> impl IntoResponse {
            response
                .requests
                .send(CapturedCallback {
                    uri: uri.to_string(),
                    content_type: headers
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned),
                    body,
                })
                .unwrap();
            sleep(response.delay).await;
            (response.status, response.response_body)
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let app = Router::new()
            .route("/api/internal/troute/jobs/{*path}", post(receive_event))
            .with_state(CallbackMock {
                status,
                response_body,
                delay,
                requests: request_tx,
            });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}"), request_rx, server)
    }
}
