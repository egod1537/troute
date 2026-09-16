use std::{
    env, error::Error, net::SocketAddr, num::NonZeroU16, path::PathBuf, process::ExitCode,
    sync::Arc,
};

use tokio::net::TcpListener;
use troute::{
    development::{DevelopmentRouteSolver, DevelopmentRoutingProvider},
    http,
    observation::JobObservationRecorder,
    storage::{FileJobStore, FileJobTimelineStore, JobStore},
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
    let job_store = Arc::new(FileJobStore::new(&data_dir)?);
    let recovered_jobs = job_store.recover_interrupted()?;
    let observation = JobObservationRecorder::new(Arc::new(FileJobTimelineStore::new(&data_dir)?));
    let address = SocketAddr::from(([0, 0, 0, 0], port));
    println!("troute server starting; bind address: {address}");
    println!("Local job data directory: {}", data_dir.display());
    println!("Recovered interrupted jobs: {recovered_jobs}");
    let listener = TcpListener::bind(address).await?;
    let optimizer =
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver);
    let app = http::router_with_storage(optimizer, Some(observation), Some(job_store));

    println!("troute server listening on http://{address}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    println!("troute server stopped");
    Ok(())
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
