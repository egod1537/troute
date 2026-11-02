use std::{
    collections::VecDeque,
    net::TcpListener,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{Request, StatusCode},
    response::Response,
    Router,
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
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Clone)]
struct ScriptedServerState {
    responses: Arc<Mutex<VecDeque<ResponseSpec>>>,
    requests: Arc<Mutex<Vec<String>>>,
}

async fn scripted_response(
    State(state): State<ScriptedServerState>,
    request: Request<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
    let mut captured = format!("{} {} HTTP/1.1\r\n", parts.method, parts.uri);
    for (name, value) in &parts.headers {
        captured.push_str(name.as_str());
        captured.push_str(": ");
        captured.push_str(value.to_str().unwrap_or("<binary>"));
        captured.push_str("\r\n");
    }
    captured.push_str("\r\n");
    captured.push_str(&String::from_utf8_lossy(&bytes));
    state.requests.lock().unwrap().push(captured);

    let spec = state
        .responses
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| spec(500, r#"{"error":"mock response script exhausted"}"#));
    tokio::time::sleep(spec.delay).await;
    Response::builder()
        .status(StatusCode::from_u16(spec.status).unwrap())
        .header("content-type", "application/json")
        .body(Body::from(spec.body))
        .unwrap()
}

impl MockServer {
    fn start(responses: Vec<ResponseSpec>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = ScriptedServerState {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
            requests: Arc::clone(&requests),
        };
        let (shutdown, shutdown_receiver) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let app = Router::new().fallback(scripted_response).with_state(state);
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown_receiver.await;
                    })
                    .await
                    .unwrap();
            });
        });
        Self {
            base_url: Url::parse(&format!("http://{address}/")).unwrap(),
            requests,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
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

include!("tests/lifecycle.rs");
include!("tests/protocol_errors.rs");
include!("tests/cancellation.rs");
