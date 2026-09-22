use std::{
    env,
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
    matrix::TravelTimeMatrix,
    routing::{RoutingContext, RoutingError, RoutingProvider},
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
        let base_url = env::var("TCACHE_BASE_URL")
            .map_err(|_| "TCACHE_BASE_URL is required when ROUTING_PROVIDER=tcache".to_owned())?;
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
        let poll_interval =
            read_positive_milliseconds("TCACHE_MATRIX_POLL_INTERVAL_MS", DEFAULT_POLL_INTERVAL_MS)?;
        let timeout = read_positive_milliseconds("TCACHE_MATRIX_TIMEOUT_MS", DEFAULT_TIMEOUT_MS)?;
        Ok(Self {
            base_url,
            poll_interval,
            timeout,
        })
    }
}

fn read_positive_milliseconds(name: &str, fallback: u64) -> Result<Duration, String> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(Duration::from_millis(fallback)),
        Err(error) => return Err(error.to_string()),
    };
    let value = value
        .parse::<NonZeroU64>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    Ok(Duration::from_millis(value.get()))
}

#[derive(Debug, Clone)]
pub struct TcacheRoutingProvider {
    client: Client,
    config: TcacheRoutingConfig,
}

impl TcacheRoutingProvider {
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
        job_id: Option<&str>,
    ) -> Result<T, RoutingError> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(timeout_error(job_id));
        }
        let response = request
            .timeout(remaining)
            .send()
            .map_err(|error| http_error(endpoint, job_id, error))?;
        decode_response(response, endpoint, job_id)
    }

    fn cancel(&self, job_id: &str) {
        let Ok(endpoint) = self.endpoint(&format!("api/route/matrix/jobs/{job_id}/cancel")) else {
            return;
        };
        let timeout = self.config.poll_interval.min(Duration::from_secs(2));
        let _ = self.client.post(endpoint).timeout(timeout).send();
    }
}

impl RoutingProvider for TcacheRoutingProvider {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let deadline = Instant::now() + self.config.timeout;
        let endpoint = self.endpoint("api/route/matrix/jobs")?;
        let request = MatrixRequest {
            locations: locations.iter().map(MatrixLocation::from).collect(),
            mode: context.travel_mode.as_provider_value(),
            departure_time: context
                .departure_time
                .unwrap_or_else(Utc::now)
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        };
        let created: CreateJobResponse = self.request_json(
            self.client.post(endpoint.clone()).json(&request),
            &endpoint,
            deadline,
            None,
        )?;
        if created.job_id.trim().is_empty() {
            return Err(RoutingError::Provider(format!(
                "tcache returned an empty jobId from {endpoint}"
            )));
        }
        let job_id = created.job_id;
        let status_endpoint = self.endpoint(&format!("api/route/matrix/jobs/{job_id}"))?;

        loop {
            if Instant::now() >= deadline {
                self.cancel(&job_id);
                eprintln!("tcache matrix job timed out; jobId={job_id}");
                return Err(timeout_error(Some(&job_id)));
            }
            let status: JobStatusResponse = match self.request_json(
                self.client.get(status_endpoint.clone()),
                &status_endpoint,
                deadline,
                Some(&job_id),
            ) {
                Ok(status) => status,
                Err(_error) if Instant::now() >= deadline => {
                    self.cancel(&job_id);
                    eprintln!("tcache matrix job timed out; jobId={job_id}");
                    return Err(timeout_error(Some(&job_id)));
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
                        "tcache matrix job {job_id} {}: {detail}",
                        status.status
                    )));
                }
                "queued" | "running" => {}
                other => {
                    return Err(RoutingError::Provider(format!(
                        "tcache matrix job {job_id} returned unknown status {other}"
                    )))
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                continue;
            }
            thread::sleep(self.config.poll_interval.min(remaining));
        }

        let result_endpoint = self.endpoint(&format!("api/route/matrix/jobs/{job_id}/result"))?;
        let result: MatrixResult = match self.request_json(
            self.client.get(result_endpoint.clone()),
            &result_endpoint,
            deadline,
            Some(&job_id),
        ) {
            Ok(result) => result,
            Err(_error) if Instant::now() >= deadline => {
                self.cancel(&job_id);
                eprintln!("tcache matrix job timed out; jobId={job_id}");
                return Err(timeout_error(Some(&job_id)));
            }
            Err(error) => return Err(error),
        };
        validate_and_convert_result(&job_id, locations, result)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MatrixRequest<'a> {
    locations: Vec<MatrixLocation<'a>>,
    mode: &'static str,
    departure_time: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MatrixLocation<'a> {
    id: &'a str,
    place_id: &'a str,
}

impl<'a> From<&'a Location> for MatrixLocation<'a> {
    fn from(location: &'a Location) -> Self {
        let RoutingReference::GooglePlaceId(place_id) = location.routing_reference();
        Self {
            id: location.id(),
            place_id,
        }
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
struct MatrixResult {
    job_id: String,
    locations: Vec<ResultLocation>,
    duration_seconds: Vec<Vec<u64>>,
}

#[derive(Deserialize)]
struct ResultLocation {
    id: String,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: TcacheErrorBody,
}

fn decode_response<T: DeserializeOwned>(
    response: Response,
    endpoint: &Url,
    job_id: Option<&str>,
) -> Result<T, RoutingError> {
    let status = response.status();
    let body = response.text().map_err(|error| {
        RoutingError::Provider(format!(
            "cannot read tcache response from {endpoint}{}: {error}",
            job_context(job_id)
        ))
    })?;
    if !status.is_success() {
        let detail = serde_json::from_str::<ErrorEnvelope>(&body)
            .map(|envelope| format!("{}: {}", envelope.error.code, envelope.error.message))
            .unwrap_or_else(|_| truncate(&body));
        return Err(RoutingError::Provider(format!(
            "tcache request to {endpoint}{} failed with HTTP {}: {detail}",
            job_context(job_id),
            status.as_u16()
        )));
    }
    serde_json::from_str(&body).map_err(|error| {
        RoutingError::Provider(format!(
            "cannot decode tcache JSON from {endpoint}{}: {error}",
            job_context(job_id)
        ))
    })
}

fn validate_and_convert_result(
    job_id: &str,
    requested: &[Location],
    result: MatrixResult,
) -> Result<TravelTimeMatrix, RoutingError> {
    if result.job_id != job_id {
        return Err(invalid_result(job_id, "result jobId does not match"));
    }
    let expected_ids: Vec<_> = requested.iter().map(Location::id).collect();
    let actual_ids: Vec<_> = result
        .locations
        .iter()
        .map(|location| location.id.as_str())
        .collect();
    if actual_ids != expected_ids {
        return Err(invalid_result(
            job_id,
            "result location ordering does not match request",
        ));
    }
    if result.duration_seconds.len() != requested.len() {
        return Err(invalid_result(
            job_id,
            "matrix row count does not match locations",
        ));
    }
    let mut rows = Vec::with_capacity(requested.len());
    for (row_index, row) in result.duration_seconds.into_iter().enumerate() {
        if row.len() != requested.len() {
            return Err(invalid_result(job_id, "matrix is not square"));
        }
        if row[row_index] != 0 {
            return Err(invalid_result(job_id, "matrix diagonal must be zero"));
        }
        rows.push(
            row.into_iter()
                .map(|seconds| {
                    let minutes = seconds.div_ceil(60);
                    u32::try_from(minutes)
                        .map_err(|_| invalid_result(job_id, "duration exceeds supported range"))
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    TravelTimeMatrix::new(rows).map_err(|error| invalid_result(job_id, &error.to_string()))
}

fn invalid_result(job_id: &str, detail: &str) -> RoutingError {
    RoutingError::Provider(format!(
        "invalid tcache matrix result for job {job_id}: {detail}"
    ))
}

fn timeout_error(job_id: Option<&str>) -> RoutingError {
    RoutingError::Provider(format!(
        "tcache matrix request timed out{}",
        job_context(job_id)
    ))
}

fn http_error(endpoint: &Url, job_id: Option<&str>, error: reqwest::Error) -> RoutingError {
    RoutingError::Provider(format!(
        "tcache HTTP request to {endpoint}{} failed: {error}",
        job_context(job_id)
    ))
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
        body: &'static str,
        delay: Duration,
    }

    struct MockServer {
        base_url: Url,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        _listener_guard: TcpListener,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl MockServer {
        fn start(responses: Vec<ResponseSpec>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            // Keep the port bound until MockServer::drop. Otherwise the worker
            // can finish first and a concurrent test may reuse the port before
            // the wake-up connection in drop runs.
            let listener_guard = listener.try_clone().unwrap();
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
                    stream.shutdown(std::net::Shutdown::Write).unwrap();
                    let mut drain = [0_u8; 256];
                    while stream.read(&mut drain).unwrap_or(0) > 0 {}
                }
            });
            Self {
                base_url: Url::parse(&format!("http://{address}/")).unwrap(),
                requests,
                stop,
                _listener_guard: listener_guard,
                thread: Some(thread),
            }
        }
    }

    impl Drop for MockServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            let _ = std::net::TcpStream::connect((
                self.base_url.host_str().unwrap(),
                self.base_url.port().unwrap(),
            ));
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

    fn spec(status: u16, body: &'static str) -> ResponseSpec {
        ResponseSpec {
            status,
            body,
            delay: Duration::ZERO,
        }
    }

    fn provider(server: &MockServer, timeout: Duration) -> TcacheRoutingProvider {
        TcacheRoutingProvider::new(TcacheRoutingConfig {
            base_url: server.base_url.clone(),
            poll_interval: Duration::from_millis(10),
            timeout,
        })
        .unwrap()
    }

    fn locations() -> Vec<Location> {
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
            .collect()
    }

    #[test]
    fn creates_polls_and_decodes_a_matrix_in_request_order() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"matrix-1","status":"queued"}"#),
            spec(200, r#"{"status":"queued"}"#),
            spec(200, r#"{"status":"running"}"#),
            spec(200, r#"{"status":"completed"}"#),
            spec(
                200,
                r#"{"jobId":"matrix-1","locations":[{"id":"A"},{"id":"B"}],"durationSeconds":[[0,61],[120,0]]}"#,
            ),
        ]);
        let matrix = provider(&server, Duration::from_secs(1))
            .travel_time_matrix(&locations(), &RoutingContext::default())
            .unwrap();
        assert_eq!(matrix.travel_minutes(0, 1), Some(2));
        assert_eq!(matrix.travel_minutes(1, 0), Some(2));
        let requests = server.requests.lock().unwrap();
        assert!(
            requests[0].starts_with("POST /api/route/matrix/jobs "),
            "{}",
            requests[0]
        );
        assert!(requests[0].contains(r#""id":"A","placeId":"place-A""#));
        assert!(requests[0].contains(r#""id":"B","placeId":"place-B""#));
        assert!(requests[0].contains(r#""mode":"TRANSIT""#));
        assert!(requests[4].starts_with("GET /api/route/matrix/jobs/matrix-1/result "));
    }

    #[test]
    fn maps_failed_and_cancelled_jobs_to_routing_errors() {
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
            let leaked: &'static str = Box::leak(body.into_boxed_str());
            let server = MockServer::start(vec![
                spec(202, r#"{"jobId":"matrix-2"}"#),
                spec(200, leaked),
            ]);
            let error = provider(&server, Duration::from_secs(1))
                .travel_time_matrix(&locations(), &RoutingContext::default())
                .unwrap_err();
            assert!(
                error.to_string().contains(code),
                "expected {code} in {error}"
            );
        }
    }

    #[test]
    fn reports_malformed_and_non_success_responses() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let malformed = MockServer::start(vec![spec(202, "not-json")]);
        let error = provider(&malformed, Duration::from_secs(1))
            .travel_time_matrix(&locations(), &RoutingContext::default())
            .unwrap_err();
        assert!(error.to_string().contains("cannot decode tcache JSON"));

        let unavailable = MockServer::start(vec![spec(
            503,
            r#"{"error":{"code":"PROVIDER_ERROR","message":"offline"}}"#,
        )]);
        let error = provider(&unavailable, Duration::from_secs(1))
            .travel_time_matrix(&locations(), &RoutingContext::default())
            .unwrap_err();
        assert!(error.to_string().contains("HTTP 503"));
        assert!(error.to_string().contains("PROVIDER_ERROR"));
    }

    #[test]
    fn rejects_location_order_and_matrix_validation_failures() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for result in [
            r#"{"jobId":"matrix-3","locations":[{"id":"B"},{"id":"A"}],"durationSeconds":[[0,60],[60,0]]}"#,
            r#"{"jobId":"matrix-3","locations":[{"id":"A"},{"id":"B"}],"durationSeconds":[[0],[60,0]]}"#,
            r#"{"jobId":"matrix-3","locations":[{"id":"A"},{"id":"B"}],"durationSeconds":[[1,60],[60,0]]}"#,
        ] {
            let server = MockServer::start(vec![
                spec(202, r#"{"jobId":"matrix-3"}"#),
                spec(200, r#"{"status":"completed"}"#),
                spec(200, result),
            ]);
            let error = provider(&server, Duration::from_secs(1))
                .travel_time_matrix(&locations(), &RoutingContext::default())
                .unwrap_err();
            assert!(error.to_string().contains("invalid tcache matrix result"));
        }
    }

    #[test]
    fn times_out_and_attempts_to_cancel_the_tcache_job() {
        let _guard = TCACHE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"matrix-timeout"}"#),
            ResponseSpec {
                status: 200,
                body: r#"{"status":"running"}"#,
                delay: Duration::from_millis(80),
            },
            spec(200, r#"{"jobId":"matrix-timeout","status":"cancelled"}"#),
        ]);
        let error = provider(&server, Duration::from_millis(30))
            .travel_time_matrix(&locations(), &RoutingContext::default())
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        std::thread::sleep(Duration::from_millis(100));
        assert!(server.requests.lock().unwrap().iter().any(
            |request| request.starts_with("POST /api/route/matrix/jobs/matrix-timeout/cancel ")
        ));
    }
}
