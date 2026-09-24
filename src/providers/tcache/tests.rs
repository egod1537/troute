use std::{
    io::{Read, Write},
    net::TcpListener,
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use reqwest::{blocking::Client, Url};

use super::*;
use crate::{
    cancellation::CancellationToken,
    domain::{Location, RoutingReference, TimeOfDay, TimeWindow},
    routing::{
        PairwiseMatrixRoutingProvider, RoutingContext, RoutingError, RoutingProvider, TravelTime,
        TravelTimeProvider,
    },
};

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
            ("routeProvider".to_owned(), "kakao-maps".to_owned()),
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
    assert!(requests[0].contains(r#""locations":[{"placeId":"place-A"},{"placeId":"place-B"}]"#));
    assert!(requests[0].contains(r#""mode":"WALKING""#));
    assert!(requests[0].contains(r#""departureTime":"2026-09-23T00:00:00Z""#));
    assert!(requests[0].contains(r#""languageCode":"ko""#));
    assert!(requests[0].contains(r#""regionCode":"KR""#));
    assert!(requests[0].contains(r#""provider":"kakao-maps""#));
    assert!(!requests[0].contains("unsupported"));
    assert!(!requests[0].contains("secret-internal-option"));
    assert!(requests[4].starts_with("GET /api/route/jobs/route-1/result "));
}

#[test]
fn forwards_every_supported_travel_mode_to_route_jobs() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (travel_mode, provider_value) in [
        (crate::routing::TravelMode::Transit, "TRANSIT"),
        (crate::routing::TravelMode::Driving, "DRIVING"),
        (crate::routing::TravelMode::Walking, "WALKING"),
        (crate::routing::TravelMode::Bicycling, "BICYCLING"),
    ] {
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"route-mode"}"#),
            spec(200, r#"{"status":"completed"}"#),
            spec(
                200,
                r#"{"jobId":"route-mode","status":"completed","result":{"routes":[{"durationSeconds":60}]}}"#,
            ),
        ]);
        let context = RoutingContext {
            travel_mode,
            ..RoutingContext::default()
        };

        travel_time(&provider(&server, Duration::from_secs(1)), &context).unwrap();

        let requests = server.requests.lock().unwrap();
        assert!(
            requests[0].contains(&format!(r#""mode":"{provider_value}""#)),
            "request did not contain mode {provider_value}: {}",
            requests[0]
        );
    }
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
        let body =
            format!(r#"{{"status":"{status}","error":{{"code":"{code}","message":"stopped"}}}}"#);
        let server = MockServer::start(vec![spec(202, r#"{"jobId":"route-2"}"#), spec(200, body)]);
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
fn preserves_tcache_provider_configuration_error_types() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (code, expected_capability_error) in [
        ("PROVIDER_NOT_CONFIGURED", false),
        ("UNSUPPORTED_PROVIDER_CAPABILITY", true),
    ] {
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"route-provider-error"}"#),
            spec(
                200,
                format!(
                    r#"{{"status":"failed","error":{{"code":"{code}","message":"provider setup failed"}}}}"#
                ),
            ),
        ]);
        let context = RoutingContext {
            travel_mode: crate::routing::TravelMode::Transit,
            options: [("routeProvider".to_owned(), "ekispert".to_owned())]
                .into_iter()
                .collect(),
            ..RoutingContext::default()
        };
        let error = travel_time(&provider(&server, Duration::from_secs(1)), &context).unwrap_err();
        if expected_capability_error {
            assert!(matches!(
                error,
                RoutingError::UnsupportedProviderCapability { ref provider, ref mode }
                    if provider == "ekispert" && mode == "TRANSIT"
            ));
        } else {
            assert!(matches!(
                error,
                RoutingError::ProviderNotConfigured { ref provider, ref reason }
                    if provider == "ekispert" && reason == "provider setup failed"
            ));
        }
    }
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

#[test]
fn cooperative_cancellation_attempts_to_cancel_the_tcache_job() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = MockServer::start(vec![
        spec(202, r#"{"jobId":"route-cancel"}"#),
        spec(200, r#"{"status":"running"}"#),
        spec(200, r#"{"jobId":"route-cancel","status":"cancelled"}"#),
    ]);
    let provider = provider(&server, Duration::from_secs(1));
    let cancellation = CancellationToken::new();
    let [from, to] = locations();

    std::thread::scope(|scope| {
        let request = scope.spawn(|| {
            provider.travel_time_with_cancellation(
                &from,
                &to,
                &RoutingContext::default(),
                &cancellation,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while server.requests.lock().unwrap().len() < 2 {
            assert!(Instant::now() < deadline, "status request did not start");
            std::thread::sleep(Duration::from_millis(1));
        }
        cancellation.cancel();

        let error = request.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    });

    assert!(server
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| request.starts_with("POST /api/route/jobs/route-cancel/cancel ")));
}

#[test]
fn matrix_total_timeout_precedes_pair_timeout_and_cancels_the_job() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = MockServer::start(vec![
        spec(202, r#"{"jobId":"route-total-timeout"}"#),
        ResponseSpec {
            status: 200,
            body: r#"{"status":"running"}"#.to_owned(),
            delay: Duration::from_millis(80),
        },
        spec(
            200,
            r#"{"jobId":"route-total-timeout","status":"cancelled"}"#,
        ),
    ]);
    let pair_provider = provider(&server, Duration::from_secs(1));
    let matrix_provider = PairwiseMatrixRoutingProvider::with_limits(
        pair_provider,
        NonZeroUsize::new(1).unwrap(),
        Duration::from_millis(30),
    );

    let error = matrix_provider
        .travel_time_matrix(&locations(), &RoutingContext::default())
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("matrix_timeout=true"), "{message}");
    assert!(message.contains("completed_pairs=0/2"), "{message}");
    assert!(message.contains("failed_pair=A -> B"), "{message}");
    std::thread::sleep(Duration::from_millis(100));
    assert!(server
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| request.starts_with("POST /api/route/jobs/route-total-timeout/cancel ")));
}

#[test]
fn pair_timeout_precedes_matrix_total_timeout() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = MockServer::start(vec![
        spec(202, r#"{"jobId":"route-pair-timeout"}"#),
        ResponseSpec {
            status: 200,
            body: r#"{"status":"running"}"#.to_owned(),
            delay: Duration::from_millis(80),
        },
        spec(
            200,
            r#"{"jobId":"route-pair-timeout","status":"cancelled"}"#,
        ),
    ]);
    let pair_provider = provider(&server, Duration::from_millis(30));
    let matrix_provider = PairwiseMatrixRoutingProvider::with_limits(
        pair_provider,
        NonZeroUsize::new(1).unwrap(),
        Duration::from_millis(300),
    );

    let error = matrix_provider
        .travel_time_matrix(&locations(), &RoutingContext::default())
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("matrix_timeout=false"), "{message}");
    assert!(message.contains("timed out"), "{message}");
    assert!(message.contains("failed_pair=A -> B"), "{message}");
}
