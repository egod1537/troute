use std::{
    env, error::Error, net::SocketAddr, num::NonZeroU16, path::PathBuf, process::ExitCode,
    sync::Arc, time::Duration,
};

use tokio::net::TcpListener;
use troute::{
    http,
    observation::JobObservationRecorder,
    routing::PairwiseMatrixRoutingProvider,
    solver::{
        MatchingStrategyConfig, SolverOrchestrator, SolverOrchestratorConfig, EXACT_MAX_LOCATIONS,
        MAX_EXACT_CLUSTER_SIZE,
    },
    storage::{FileJobStore, FileJobTimelineStore, JobStore},
    tcache::{TcacheRoutingConfig, TcacheTravelTimeProvider},
    RouteOptimizationService,
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("troute server error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let port = match env::var("TROUTE_PORT") {
        Ok(value) => value
            .parse::<NonZeroU16>()
            .map_err(|_| "TROUTE_PORT must be an integer between 1 and 65535")?
            .get(),
        Err(env::VarError::NotPresent) => 8080,
        Err(error) => return Err(error.into()),
    };
    let data_dir = match env::var("TROUTE_DATA_DIR") {
        Ok(value) if !value.trim().is_empty() => PathBuf::from(value),
        Ok(_) => return Err("TROUTE_DATA_DIR must not be empty".into()),
        Err(env::VarError::NotPresent) => PathBuf::from(".local/troute"),
        Err(error) => return Err(error.into()),
    };
    let travel_time_provider = TcacheTravelTimeProvider::new(
        TcacheRoutingConfig::from_env()
            .map_err(|error| format!("invalid tcache configuration: {error}"))?,
    )?;
    let routing_provider = PairwiseMatrixRoutingProvider::new(travel_time_provider);
    let exact_limit = read_bounded_size(
        "TROUTE_EXACT_LIMIT",
        EXACT_MAX_LOCATIONS,
        EXACT_MAX_LOCATIONS,
    )?;
    let max_cluster_size = read_bounded_size(
        "TROUTE_MAX_EXACT_CLUSTER_SIZE",
        troute::solver::DEFAULT_MAX_EXACT_CLUSTER_SIZE,
        MAX_EXACT_CLUSTER_SIZE,
    )?;
    let default_orchestrator = SolverOrchestratorConfig::default();
    let max_concurrency = read_bounded_size(
        "SOLVER_MAX_CONCURRENCY",
        default_orchestrator.max_concurrency,
        256,
    )?;
    let strategy_timeout_ms = read_positive_u64(
        "SOLVER_STRATEGY_TIMEOUT_MS",
        u64::try_from(default_orchestrator.strategy_timeout.as_millis()).unwrap_or(u64::MAX),
    )?;
    let sa_seeds = read_u64_list("SOLVER_SA_SEEDS", &default_orchestrator.sa_seeds)?;
    let solver = SolverOrchestrator::new(SolverOrchestratorConfig {
        exact_limit,
        max_cluster_size,
        max_concurrency,
        strategy_timeout: Duration::from_millis(strategy_timeout_ms),
        sa_seeds,
        matching: MatchingStrategyConfig::from_env()?,
        ..default_orchestrator
    })?;
    let job_store = Arc::new(FileJobStore::new(&data_dir)?);
    let recovered_jobs = job_store.recover_interrupted()?;
    let observation = JobObservationRecorder::new(Arc::new(FileJobTimelineStore::new(&data_dir)?));
    let address = SocketAddr::from(([0, 0, 0, 0], port));
    println!("troute server starting; bind address: {address}");
    println!("Local job data directory: {}", data_dir.display());
    println!("Recovered interrupted jobs: {recovered_jobs}");
    println!("Exact solver location limit: {exact_limit}");
    println!("Maximum exact cluster size: {max_cluster_size}");
    println!("Solver strategy concurrency: {max_concurrency}");
    println!("Per-strategy timeout: {strategy_timeout_ms} ms");
    let listener = TcpListener::bind(address).await?;
    let optimizer = RouteOptimizationService::new(routing_provider, solver);
    let app = http::router_with_storage(optimizer, Some(observation), Some(job_store));

    println!("troute server listening on http://{address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    println!("troute server stopped");
    Ok(())
}

fn read_bounded_size(name: &str, default: usize, maximum: usize) -> Result<usize, String> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(default),
        Err(error) => return Err(error.to_string()),
    };
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{name} must be an integer between 1 and {maximum}"))?;
    if !(1..=maximum).contains(&parsed) {
        return Err(format!("{name} must be an integer between 1 and {maximum}"));
    }
    Ok(parsed)
}

fn read_positive_u64(name: &str, default: u64) -> Result<u64, String> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(default),
        Err(error) => return Err(error.to_string()),
    };
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{name} must be a positive integer"));
    }
    Ok(parsed)
}

fn read_u64_list(name: &str, default: &[u64]) -> Result<Vec<u64>, String> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(default.to_vec()),
        Err(error) => return Err(error.to_string()),
    };
    let values: Vec<_> = value
        .split(',')
        .map(str::trim)
        .map(|item| {
            item.parse::<u64>()
                .map_err(|_| format!("{name} must be a comma-separated list of u64 seeds"))
        })
        .collect::<Result<_, _>>()?;
    if values.is_empty() {
        return Err(format!("{name} must contain at least one seed"));
    }
    Ok(values)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            eprintln!("troute server error: cannot listen for Ctrl-C: {error}");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => eprintln!("troute server error: cannot listen for SIGTERM: {error}"),
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    println!("troute server shutting down");
}
