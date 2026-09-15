use std::time::Duration;

use reqwest::{redirect::Policy, StatusCode, Url};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::events::JobEvent;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_PATH: &str = "api/internal/troute/health";
const JOBS_PATH: &str = "api/internal/troute/jobs";

/// Reusable asynchronous client for troute-to-Trasolve communication.
#[derive(Clone, Debug)]
pub struct TrasolveClient {
    base_url: String,
    health_url: Url,
    jobs_url: Url,
    http: reqwest::Client,
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
        })
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

        let response = self
            .http
            .post(url)
            .json(event)
            .send()
            .await
            .map_err(classify_request_error)?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response
                .text()
                .await
                .map_err(classify_request_error)
                .map(|body| (!body.is_empty()).then_some(body))?;
            return Err(TrasolveClientError::UpstreamHttp { status, body });
        }

        Ok(())
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
    use crate::events::{JobEvent, JobEventType};
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
