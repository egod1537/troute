use super::*;

/// Builds the HTTP application around an existing optimization service.
pub fn router<P, S>(optimizer: RouteOptimizationService<P, S>) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    router_with_storage(optimizer, None, None)
}

/// Builds the HTTP application with optional testbed observation support.
pub fn router_with_observation<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    observation: Option<JobObservationRecorder>,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    router_with_storage(optimizer, observation, None)
}

/// Builds the HTTP application with persistent job and timeline storage.
pub fn router_with_storage<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    router_with_storage_options(
        optimizer,
        observation,
        job_store,
        max_concurrent_jobs_from_env(),
        DEFAULT_SSE_HEARTBEAT_INTERVAL,
    )
}

/// Builds the HTTP application with an explicit background-job concurrency limit.
pub fn router_with_storage_and_limit<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
    max_concurrent_jobs: Option<usize>,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    router_with_storage_options(
        optimizer,
        observation,
        job_store,
        max_concurrent_jobs,
        DEFAULT_SSE_HEARTBEAT_INTERVAL,
    )
}

/// Builds the HTTP application with explicit background-job and SSE settings.
pub fn router_with_storage_options<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
    max_concurrent_jobs: Option<usize>,
    sse_heartbeat_interval: Duration,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    let options = RouterRuntimeOptions {
        max_concurrent_jobs,
        max_outstanding_jobs: Some(max_outstanding_jobs_from_env()),
        max_concurrent_matrix_previews: max_matrix_previews_from_env(),
        max_routing_pairs: max_routing_pairs_from_env(),
        sse_heartbeat_interval,
    };
    router_with_storage_runtime_options(optimizer, observation, job_store, options)
}

/// Builds the HTTP application from fully resolved process-level limits.
///
/// Unlike the compatibility constructors, this function performs no
/// environment lookup and is the entry point used by the production server.
pub fn router_with_storage_runtime_options<P, S>(
    optimizer: RouteOptimizationService<P, S>,
    observation: Option<JobObservationRecorder>,
    job_store: Option<Arc<dyn JobStore>>,
    options: RouterRuntimeOptions,
) -> Router
where
    P: RoutingProvider + Send + Sync + 'static,
    S: RouteSolver + Send + Sync + 'static,
{
    let RouterRuntimeOptions {
        max_concurrent_jobs,
        max_outstanding_jobs,
        max_concurrent_matrix_previews,
        max_routing_pairs,
        sse_heartbeat_interval,
    } = options;
    let executor: Arc<dyn JobExecutor> = Arc::new(optimizer);
    let runner = job_store.as_ref().map(|store| {
        JobRunner::new_with_limits(
            executor.clone(),
            store.clone(),
            max_concurrent_jobs,
            max_outstanding_jobs,
        )
    });
    Router::new()
        .route("/health", get(handlers::health))
        .route("/route/providers/policy", get(handlers::provider_policy))
        .route(
            "/integration/solver-capabilities",
            get(handlers::solver_capabilities),
        )
        .route("/optimize", post(handlers::optimize))
        .route("/validate", post(handlers::validate_route))
        .route(
            "/integration/jobs",
            get(handlers::list_jobs).post(handlers::submit_job),
        )
        .route("/integration/matrix", post(handlers::build_matrix))
        .route("/integration/jobs/{job_id}", get(handlers::get_job))
        .route(
            "/integration/jobs/{job_id}/solver-telemetry",
            get(handlers::solver_telemetry),
        )
        .route(
            "/integration/jobs/{job_id}/solver-events",
            get(handlers::solver_events),
        )
        .route(
            "/integration/jobs/{job_id}/events",
            get(handlers::job_events),
        )
        .route(
            "/integration/jobs/{job_id}/cancel",
            post(handlers::cancel_job),
        )
        .route(
            "/integration/jobs/{job_id}/timeline",
            get(handlers::job_timeline),
        )
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .with_state(AppState {
            executor,
            runner,
            observation,
            job_store,
            sse_heartbeat_interval: sse_heartbeat_interval.max(Duration::from_millis(1)),
            matrix_permits: Arc::new(Semaphore::new(max_concurrent_matrix_previews)),
            max_routing_pairs,
        })
}

fn max_concurrent_jobs_from_env() -> Option<usize> {
    match std::env::var("TROUTE_MAX_CONCURRENT_JOBS") {
        Ok(value) => match value.parse::<usize>() {
            Ok(limit) if limit > 0 => Some(limit),
            _ => {
                logger().invalid_environment_value(
                    "TROUTE_MAX_CONCURRENT_JOBS",
                    &value,
                    DEFAULT_MAX_CONCURRENT_JOBS,
                );
                Some(DEFAULT_MAX_CONCURRENT_JOBS)
            }
        },
        Err(std::env::VarError::NotPresent) => Some(DEFAULT_MAX_CONCURRENT_JOBS),
        Err(error) => {
            logger().environment_read_failed(
                "TROUTE_MAX_CONCURRENT_JOBS",
                &error,
                DEFAULT_MAX_CONCURRENT_JOBS,
            );
            Some(DEFAULT_MAX_CONCURRENT_JOBS)
        }
    }
}

fn max_routing_pairs_from_env() -> usize {
    match std::env::var("TROUTE_MAX_ROUTING_PAIRS") {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| *limit > 0)
            .unwrap_or_else(|| {
                logger().invalid_environment_value(
                    "TROUTE_MAX_ROUTING_PAIRS",
                    &value,
                    DEFAULT_MAX_ROUTING_PAIRS,
                );
                DEFAULT_MAX_ROUTING_PAIRS
            }),
        _ => DEFAULT_MAX_ROUTING_PAIRS,
    }
}

fn max_outstanding_jobs_from_env() -> usize {
    positive_limit_from_env("TROUTE_MAX_OUTSTANDING_JOBS", DEFAULT_MAX_OUTSTANDING_JOBS)
}

fn max_matrix_previews_from_env() -> usize {
    positive_limit_from_env(
        "TROUTE_MAX_CONCURRENT_MATRIX_PREVIEWS",
        DEFAULT_MAX_CONCURRENT_MATRIX_PREVIEWS,
    )
}

fn positive_limit_from_env(name: &str, default: usize) -> usize {
    match std::env::var(name) {
        Ok(value) => value
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .unwrap_or_else(|| {
                logger().invalid_environment_value(name, &value, default);
                default
            }),
        Err(_) => default,
    }
}

pub(super) fn enforce_routing_pair_limit(
    request: &OptimizeRouteRequest,
    maximum: usize,
) -> Result<(), ApiError> {
    if request.travel_time_matrix.is_some()
        || request.sparse_travel_time_matrix.is_some()
        || request.use_euclidean_matrix
    {
        return Ok(());
    }
    let location_count = request.locations.len();
    let pairs = location_count.saturating_mul(location_count.saturating_sub(1));
    if pairs > maximum {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "ROUTING_PAIR_LIMIT_EXCEEDED",
            "The request would require too many directed routing queries.",
            format!("requested_pairs={pairs}; maximum_pairs={maximum}; locations={location_count}"),
        ));
    }
    Ok(())
}
