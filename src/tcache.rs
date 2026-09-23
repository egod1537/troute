use std::{
    env,
    error::Error,
    num::NonZeroU64,
    thread,
    time::{Duration, Instant},
};

use chrono::{SecondsFormat, Utc};
use reqwest::{
    blocking::{Client, Response},
    Url,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{
    domain::{Location, RoutingReference},
    routing::{RoutingContext, RoutingError, TravelTime, TravelTimeProvider},
};

const DEFAULT_POLL_INTERVAL_MS: u64 = 250;
const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const MAX_ERROR_BODY_CHARACTERS: usize = 2_048;

#[derive(Debug, Clone)]
pub struct TcacheRoutingConfig {
    pub base_url: Url,
    pub poll_interval: Duration,
    pub timeout: Duration,
}

impl TcacheRoutingConfig {
    pub fn from_env() -> Result<Self, String> {
        let base_url =
            env::var("TCACHE_BASE_URL").map_err(|_| "TCACHE_BASE_URL is required".to_owned())?;
        if base_url.trim().is_empty() {
            return Err("TCACHE_BASE_URL must not be empty".to_owned());
        }
        let mut base_url = Url::parse(&base_url)
            .map_err(|error| format!("TCACHE_BASE_URL must be a valid URL: {error}"))?;
        if base_url.scheme() != "http" && base_url.scheme() != "https" {
            return Err("TCACHE_BASE_URL must use http or https".to_owned());
        }
        if !base_url.path().ends_with('/') {
            let path = format!("{}/", base_url.path());
            base_url.set_path(&path);
        }
        let poll_interval = read_positive_milliseconds(
            "TCACHE_POLL_INTERVAL_MS",
            "TCACHE_MATRIX_POLL_INTERVAL_MS",
            DEFAULT_POLL_INTERVAL_MS,
        )?;
        let timeout = read_positive_milliseconds(
            "TCACHE_REQUEST_TIMEOUT_MS",
            "TCACHE_MATRIX_TIMEOUT_MS",
            DEFAULT_TIMEOUT_MS,
        )?;
        Ok(Self {
            base_url,
            poll_interval,
            timeout,
        })
    }
}

fn read_positive_milliseconds(
    name: &str,
    legacy_name: &str,
    fallback: u64,
) -> Result<Duration, String> {
    let (name, value) = match env::var(name) {
        Ok(value) => (name, value),
        Err(env::VarError::NotPresent) => match env::var(legacy_name) {
            Ok(value) => (legacy_name, value),
            Err(env::VarError::NotPresent) => return Ok(Duration::from_millis(fallback)),
            Err(error) => return Err(error.to_string()),
        },
        Err(error) => return Err(error.to_string()),
    };
    let value = value
        .parse::<NonZeroU64>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    Ok(Duration::from_millis(value.get()))
}

#[derive(Debug, Clone)]
pub struct TcacheTravelTimeProvider {
    client: Client,
    config: TcacheRoutingConfig,
}

impl TcacheTravelTimeProvider {
    pub fn new(config: TcacheRoutingConfig) -> Result<Self, RoutingError> {
        let client = Client::builder()
            .connect_timeout(config.timeout)
            .timeout(config.timeout)
            .build()
            .map_err(|error| {
                RoutingError::Provider(format!("cannot build tcache HTTP client: {error}"))
            })?;
        Ok(Self { client, config })
    }

    fn endpoint(&self, path: &str) -> Result<Url, RoutingError> {
        self.config.base_url.join(path).map_err(|error| {
            RoutingError::Provider(format!("cannot build tcache endpoint {path}: {error}"))
        })
    }

    fn request_json<T: DeserializeOwned>(
        &self,
        request: reqwest::blocking::RequestBuilder,
        endpoint: &Url,
        deadline: Instant,
        from: &Location,
        to: &Location,
        job_id: Option<&str>,
    ) -> Result<T, RoutingError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(timeout_error(from, to, job_id));
        }
        let response = match request.timeout(remaining).send() {
            Ok(response) => response,
            Err(_error) if Instant::now() >= deadline => {
                return Err(timeout_error(from, to, job_id));
            }
            Err(error) => return Err(http_error(endpoint, from, to, job_id, error)),
        };
        decode_response(response, endpoint, from, to, job_id)
    }

    fn cancel(&self, job_id: &str) {
        let Ok(endpoint) = self.endpoint(&format!("api/route/jobs/{job_id}/cancel")) else {
            return;
        };
        let timeout = self.config.poll_interval.min(Duration::from_secs(2));
        let _ = self.client.post(endpoint).timeout(timeout).send();
    }
}

impl TravelTimeProvider for TcacheTravelTimeProvider {
    fn travel_time(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
    ) -> Result<TravelTime, RoutingError> {
        let deadline = Instant::now() + self.config.timeout;
        let endpoint = self.endpoint("api/route/jobs")?;
        let request = RouteRequest {
            locations: vec![RouteLocation::from(from), RouteLocation::from(to)],
            mode: context.travel_mode.as_provider_value(),
            departure_time: context
                .departure_time
                .unwrap_or_else(Utc::now)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
            language_code: context.options.get("languageCode").map(String::as_str),
            region_code: context.options.get("regionCode").map(String::as_str),
            routing_preference: context.options.get("routingPreference").map(String::as_str),
            units: context.options.get("units").map(String::as_str),
        };
        let created: CreateJobResponse = self.request_json(
            self.client.post(endpoint.clone()).json(&request),
            &endpoint,
            deadline,
            from,
            to,
            None,
        )?;
        if created.job_id.trim().is_empty() {
            return Err(RoutingError::Provider(format!(
                "tcache returned an empty jobId from {endpoint}{}",
                pair_context(from, to)
            )));
        }
        let job_id = created.job_id;
        let status_endpoint = self.endpoint(&format!("api/route/jobs/{job_id}"))?;

        loop {
            if Instant::now() >= deadline {
                self.cancel(&job_id);
                eprintln!(
                    "tcache route job timed out; pair={} -> {}; jobId={job_id}",
                    from.id(),
                    to.id()
                );
                return Err(timeout_error(from, to, Some(&job_id)));
            }
            let status: JobStatusResponse = match self.request_json(
                self.client.get(status_endpoint.clone()),
                &status_endpoint,
                deadline,
                from,
                to,
                Some(&job_id),
            ) {
                Ok(status) => status,
                Err(_error) if Instant::now() >= deadline => {
                    self.cancel(&job_id);
                    eprintln!(
                        "tcache route job timed out; pair={} -> {}; jobId={job_id}",
                        from.id(),
                        to.id()
                    );
                    return Err(timeout_error(from, to, Some(&job_id)));
                }
                Err(error) => return Err(error),
            };
            match status.status.as_str() {
                "completed" => break,
                "failed" | "cancelled" => {
                    let detail = status
                        .error
                        .map(|error| format!("{}: {}", error.code, error.message))
                        .unwrap_or_else(|| status.status.clone());
                    return Err(RoutingError::Provider(format!(
                        "tcache route job {job_id} {}{pair}: {detail}",
                        status.status,
                        pair = pair_context(from, to),
                    )));
                }
                "queued" | "running" => {}
                other => {
                    return Err(RoutingError::Provider(format!(
                        "tcache route job {job_id} returned unknown status {other}{}",
                        pair_context(from, to)
                    )))
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                continue;
            }
            thread::sleep(self.config.poll_interval.min(remaining));
        }

        let result_endpoint = self.endpoint(&format!("api/route/jobs/{job_id}/result"))?;
        let result: RouteResultEnvelope = match self.request_json(
            self.client.get(result_endpoint.clone()),
            &result_endpoint,
            deadline,
            from,
            to,
            Some(&job_id),
        ) {
            Ok(result) => result,
            Err(_error) if Instant::now() >= deadline => {
                self.cancel(&job_id);
                eprintln!(
                    "tcache route job timed out; pair={} -> {}; jobId={job_id}",
                    from.id(),
                    to.id()
                );
                return Err(timeout_error(from, to, Some(&job_id)));
            }
            Err(error) => return Err(error),
        };
        extract_travel_time(&job_id, from, to, result)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteRequest<'a> {
    locations: Vec<RouteLocation<'a>>,
    mode: &'static str,
    departure_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    language_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    region_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    routing_preference: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    units: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RouteLocation<'a> {
    place_id: &'a str,
}

impl<'a> From<&'a Location> for RouteLocation<'a> {
    fn from(location: &'a Location) -> Self {
        let RoutingReference::GooglePlaceId(place_id) = location.routing_reference();
        Self { place_id }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateJobResponse {
    job_id: String,
}

#[derive(Deserialize)]
struct JobStatusResponse {
    status: String,
    error: Option<TcacheErrorBody>,
}

#[derive(Deserialize)]
struct TcacheErrorBody {
    code: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteResultEnvelope {
    job_id: String,
    status: String,
    result: Option<RouteResult>,
}

#[derive(Deserialize)]
struct RouteResult {
    routes: Vec<RouteResultItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteResultItem {
    duration_seconds: Option<f64>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: TcacheErrorBody,
}

fn decode_response<T: DeserializeOwned>(
    response: Response,
    endpoint: &Url,
    from: &Location,
    to: &Location,
    job_id: Option<&str>,
) -> Result<T, RoutingError> {
    let status = response.status();
    let body = response.text().map_err(|error| {
        RoutingError::Provider(format!(
            "cannot read tcache response from {endpoint}{}{}: {error}",
            pair_context(from, to),
            job_context(job_id)
        ))
    })?;
    if !status.is_success() {
        let detail = serde_json::from_str::<ErrorEnvelope>(&body)
            .map(|envelope| format!("{}: {}", envelope.error.code, envelope.error.message))
            .unwrap_or_else(|_| truncate(&body));
        return Err(RoutingError::Provider(format!(
            "tcache request to {endpoint}{}{} failed with HTTP {}: {detail}",
            pair_context(from, to),
            job_context(job_id),
            status.as_u16()
        )));
    }
    serde_json::from_str(&body).map_err(|error| {
        RoutingError::Provider(format!(
            "cannot decode tcache JSON from {endpoint}{}{}: {error}",
            pair_context(from, to),
            job_context(job_id)
        ))
    })
}

fn extract_travel_time(
    job_id: &str,
    from: &Location,
    to: &Location,
    envelope: RouteResultEnvelope,
) -> Result<TravelTime, RoutingError> {
    if envelope.job_id != job_id {
        return Err(invalid_result(
            job_id,
            from,
            to,
            "result jobId does not match",
        ));
    }
    if envelope.status != "completed" {
        return Err(invalid_result(
            job_id,
            from,
            to,
            "result status is not completed",
        ));
    }
    let seconds = envelope
        .result
        .and_then(|result| result.routes.into_iter().next())
        .and_then(|route| route.duration_seconds)
        .ok_or_else(|| invalid_result(job_id, from, to, "missing route durationSeconds"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(invalid_result(
            job_id,
            from,
            to,
            "invalid route durationSeconds",
        ));
    }
    let minutes = (seconds / 60.0).ceil();
    if minutes > f64::from(u32::MAX) {
        return Err(invalid_result(
            job_id,
            from,
            to,
            "duration exceeds supported range",
        ));
    }
    Ok(TravelTime {
        minutes: minutes as u32,
    })
}

fn invalid_result(job_id: &str, from: &Location, to: &Location, detail: &str) -> RoutingError {
    RoutingError::Provider(format!(
        "invalid tcache route result for job {job_id}{}: {detail}",
        pair_context(from, to)
    ))
}

fn timeout_error(from: &Location, to: &Location, job_id: Option<&str>) -> RoutingError {
    RoutingError::Provider(format!(
        "tcache route request timed out{}{}",
        pair_context(from, to),
        job_context(job_id),
    ))
}

fn pair_context(from: &Location, to: &Location) -> String {
    format!(" for pair {} -> {}", from.id(), to.id())
}

fn http_error(
    endpoint: &Url,
    from: &Location,
    to: &Location,
    job_id: Option<&str>,
    error: reqwest::Error,
) -> RoutingError {
    RoutingError::Provider(format!(
        "tcache HTTP request to {endpoint}{}{} failed: {}",
        pair_context(from, to),
        job_context(job_id),
        error_with_causes(&error),
    ))
}

fn error_with_causes(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    message
}

fn job_context(job_id: Option<&str>) -> String {
    job_id
        .map(|id| format!(" for job {id}"))
        .unwrap_or_default()
}

fn truncate(value: &str) -> String {
    value.chars().take(MAX_ERROR_BODY_CHARACTERS).collect()
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        },
    };

    use super::*;
    use crate::domain::{TimeOfDay, TimeWindow};

    static TCACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    struct ResponseSpec {
        status: u16,
        body: String,
        delay: Duration,
    }

    struct MockServer {
        base_url: Url,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl MockServer {
        fn start(responses: Vec<ResponseSpec>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let captured = Arc::clone(&requests);
            let stop = Arc::new(AtomicBool::new(false));
            let stopped = Arc::clone(&stop);
            let thread = std::thread::spawn(move || {
                let mut responses = responses.into_iter();
                while !stopped.load(Ordering::Relaxed) {
                    let Some(spec) = responses.next() else { break };
                    let (mut stream, request) = loop {
                        let (mut stream, _) = loop {
                            match listener.accept() {
                                Ok(connection) => break connection,
                                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                    if stopped.load(Ordering::Relaxed) {
                                        return;
                                    }
                                    std::thread::sleep(Duration::from_millis(1));
                                }
                                Err(error) => panic!("mock server accept failed: {error}"),
                            }
                        };
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let request = read_request(&mut stream);
                        if !request.is_empty() {
                            break (stream, request);
                        }
                    };
                    captured.lock().unwrap().push(request);
                    std::thread::sleep(spec.delay);
                    let reason = if spec.status >= 400 { "Error" } else { "OK" };
                    let response = format!(
                        "HTTP/1.0 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        spec.status,
                        reason,
                        spec.body.len(),
                        spec.body
                    );
                    stream
                        .write_all(response.as_bytes())
                        .expect("mock server response write failed");
                    stream.flush().unwrap();
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    let mut drain = [0_u8; 256];
                    while stream.read(&mut drain).unwrap_or(0) > 0 {}
                }
            });
            Self {
                base_url: Url::parse(&format!("http://{address}/")).unwrap(),
                requests,
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0; 1024];
        let mut expected = None;
        loop {
            let count = stream.read(&mut buffer).unwrap_or(0);
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if expected.is_none() {
                if let Some(header_end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    expected = Some(header_end + 4 + content_length);
                }
            }
            if expected.is_some_and(|length| bytes.len() >= length) {
                break;
            }
        }
        String::from_utf8(bytes).unwrap()
    }

    fn spec(status: u16, body: impl Into<String>) -> ResponseSpec {
        ResponseSpec {
            status,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    fn provider(server: &MockServer, timeout: Duration) -> TcacheTravelTimeProvider {
        let config = TcacheRoutingConfig {
            base_url: server.base_url.clone(),
            poll_interval: Duration::from_millis(10),
            timeout,
        };
        let client = Client::builder()
            .connect_timeout(timeout)
            .timeout(timeout)
            .pool_max_idle_per_host(0)
            .build()
            .unwrap();
        TcacheTravelTimeProvider { client, config }
    }

    fn locations() -> [Location; 2] {
        let window = TimeWindow::new(
            TimeOfDay::from_minutes(0).unwrap(),
            TimeOfDay::from_minutes(23 * 60 + 59).unwrap(),
        )
        .unwrap();
        ["A", "B"]
            .into_iter()
            .map(|id| {
                Location::new(
                    id.to_owned(),
                    RoutingReference::GooglePlaceId(format!("place-{id}")),
                    window,
                    0,
                )
            })
            .collect::<Vec<_>>()
            .try_into()
            .unwrap()
    }

    fn travel_time(
        provider: &TcacheTravelTimeProvider,
        context: &RoutingContext,
    ) -> Result<TravelTime, RoutingError> {
        let [from, to] = locations();
        provider.travel_time(&from, &to, context)
    }

    #[test]
    fn creates_polls_and_decodes_a_pair_route() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"route-1","status":"queued"}"#),
            spec(200, r#"{"status":"queued"}"#),
            spec(200, r#"{"status":"running"}"#),
            spec(200, r#"{"status":"completed"}"#),
            spec(
                200,
                r#"{"jobId":"route-1","status":"completed","result":{"provider":"google","routes":[{"durationSeconds":61.1}]}}"#,
            ),
        ]);
        let departure_time = "2026-09-23T09:00:00+09:00".parse().unwrap();
        let context = RoutingContext {
            departure_time: Some(departure_time),
            travel_mode: crate::routing::TravelMode::Walking,
            options: [
                ("languageCode".to_owned(), "ko".to_owned()),
                ("regionCode".to_owned(), "KR".to_owned()),
                (
                    "unsupported".to_owned(),
                    "secret-internal-option".to_owned(),
                ),
            ]
            .into_iter()
            .collect(),
            ..RoutingContext::default()
        };
        let travel = travel_time(&provider(&server, Duration::from_secs(1)), &context).unwrap();
        assert_eq!(travel.minutes, 2);
        let requests = server.requests.lock().unwrap();
        assert!(requests[0].starts_with("POST /api/route/jobs "));
        assert!(
            requests[0].contains(r#""locations":[{"placeId":"place-A"},{"placeId":"place-B"}]"#)
        );
        assert!(requests[0].contains(r#""mode":"WALKING""#));
        assert!(requests[0].contains(r#""departureTime":"2026-09-23T00:00:00Z""#));
        assert!(requests[0].contains(r#""languageCode":"ko""#));
        assert!(requests[0].contains(r#""regionCode":"KR""#));
        assert!(!requests[0].contains("unsupported"));
        assert!(!requests[0].contains("secret-internal-option"));
        assert!(requests[4].starts_with("GET /api/route/jobs/route-1/result "));
    }

    #[test]
    fn decodes_cached_route_results() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"route-cache"}"#),
            spec(200, r#"{"status":"completed","cache":{"hit":true}}"#),
            spec(
                200,
                r#"{"jobId":"route-cache","status":"completed","cache":{"hit":true},"result":{"routes":[{"durationSeconds":120}]}}"#,
            ),
        ]);
        let travel = travel_time(
            &provider(&server, Duration::from_secs(1)),
            &RoutingContext::default(),
        )
        .unwrap();
        assert_eq!(travel.minutes, 2);
    }

    #[test]
    fn maps_failed_cancelled_and_unknown_jobs_to_routing_errors() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (status, code) in [
            ("failed", "PAIR_ROUTE_FAILED"),
            ("cancelled", "JOB_CANCELLED"),
        ] {
            let body = format!(
                r#"{{"status":"{status}","error":{{"code":"{code}","message":"stopped"}}}}"#
            );
            let server =
                MockServer::start(vec![spec(202, r#"{"jobId":"route-2"}"#), spec(200, body)]);
            let error = travel_time(
                &provider(&server, Duration::from_secs(1)),
                &RoutingContext::default(),
            )
            .unwrap_err();
            assert!(
                error.to_string().contains(code),
                "expected {code} in {error}"
            );
            assert!(error.to_string().contains("A -> B"));
        }

        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"route-unknown"}"#),
            spec(200, r#"{"status":"paused"}"#),
        ]);
        let error = travel_time(
            &provider(&server, Duration::from_secs(1)),
            &RoutingContext::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown status paused"));
    }

    #[test]
    fn reports_malformed_create_status_and_result_responses() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for responses in [
            vec![spec(202, "not-json")],
            vec![
                spec(202, r#"{"jobId":"route-status"}"#),
                spec(200, r#"{"stage":"running"}"#),
            ],
            vec![
                spec(202, r#"{"jobId":"route-result"}"#),
                spec(200, r#"{"status":"completed"}"#),
                spec(200, "not-json"),
            ],
        ] {
            let server = MockServer::start(responses);
            let error = travel_time(
                &provider(&server, Duration::from_secs(1)),
                &RoutingContext::default(),
            )
            .unwrap_err();
            assert!(error.to_string().contains("cannot decode tcache JSON"));
        }
    }

    #[test]
    fn reports_empty_job_id_and_non_success_responses() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let empty = MockServer::start(vec![spec(202, r#"{"jobId":""}"#)]);
        let error = travel_time(
            &provider(&empty, Duration::from_secs(1)),
            &RoutingContext::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("empty jobId"));

        let unavailable = MockServer::start(vec![spec(
            503,
            r#"{"error":{"code":"PROVIDER_ERROR","message":"offline"}}"#,
        )]);
        let error = travel_time(
            &provider(&unavailable, Duration::from_secs(1)),
            &RoutingContext::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("HTTP 503"));
        assert!(error.to_string().contains("PROVIDER_ERROR"));
    }

    #[test]
    fn reports_unreachable_tcache_with_pair_context() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let provider = TcacheTravelTimeProvider::new(TcacheRoutingConfig {
            base_url: Url::parse(&format!("http://{address}/")).unwrap(),
            poll_interval: Duration::from_millis(10),
            timeout: Duration::from_millis(100),
        })
        .unwrap();
        let error = travel_time(&provider, &RoutingContext::default()).unwrap_err();
        assert!(error.to_string().contains("tcache HTTP request"));
        assert!(error.to_string().contains("A -> B"));
    }

    #[test]
    fn rejects_missing_and_invalid_route_durations() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (result, expected) in [
            (
                r#"{"jobId":"route-3","status":"completed","result":{"routes":[]}}"#,
                "missing route durationSeconds",
            ),
            (
                r#"{"jobId":"route-3","status":"completed","result":{"routes":[{"durationSeconds":null}]}}"#,
                "missing route durationSeconds",
            ),
            (
                r#"{"jobId":"route-3","status":"completed","result":{"routes":[{"durationSeconds":-1}]}}"#,
                "invalid route durationSeconds",
            ),
        ] {
            let server = MockServer::start(vec![
                spec(202, r#"{"jobId":"route-3"}"#),
                spec(200, r#"{"status":"completed"}"#),
                spec(200, result),
            ]);
            let error = travel_time(
                &provider(&server, Duration::from_secs(1)),
                &RoutingContext::default(),
            )
            .unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn times_out_and_attempts_to_cancel_the_tcache_job() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"route-timeout"}"#),
            ResponseSpec {
                status: 200,
                body: r#"{"status":"running"}"#.to_owned(),
                delay: Duration::from_millis(80),
            },
            spec(200, r#"{"jobId":"route-timeout","status":"cancelled"}"#),
        ]);
        let error = travel_time(
            &provider(&server, Duration::from_millis(30)),
            &RoutingContext::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        std::thread::sleep(Duration::from_millis(100));
        assert!(server
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.starts_with("POST /api/route/jobs/route-timeout/cancel ")));
    }
}
