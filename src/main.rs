use std::{env, error::Error, net::SocketAddr, num::NonZeroU16, process::ExitCode, sync::Arc};

use tokio::net::TcpListener;
use troute::{
    development::{DevelopmentRouteSolver, DevelopmentRoutingProvider},
    http,
    observation::{InMemoryJobTimelineStore, JobObservationRecorder},
    trasolve::TrasolveClient,
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
    let trasolve = match env::var("TRASOLVE_BASE_URL") {
        Ok(base_url) => Some(TrasolveClient::new(base_url)?),
        Err(env::VarError::NotPresent) => None,
        Err(error) => return Err(error.into()),
    };
    let observation_enabled = match env::var("TROUTE_ENABLE_TESTBED_OBSERVATION") {
        Ok(value) => value
            .parse::<bool>()
            .map_err(|_| "TROUTE_ENABLE_TESTBED_OBSERVATION must be `true` or `false`")?,
        Err(env::VarError::NotPresent) => false,
        Err(error) => return Err(error.into()),
    };
    let observation = observation_enabled
        .then(|| JobObservationRecorder::new(Arc::new(InMemoryJobTimelineStore::default())));
    let address = SocketAddr::from(([0, 0, 0, 0], port));
    println!("troute server starting; bind address: {address}");
    println!(
        "Trasolve reverse integration: {}",
        if trasolve.is_some() {
            "configured"
        } else {
            "not configured"
        }
    );
    println!(
        "Testbed job observation: {}",
        if observation_enabled {
            "enabled"
        } else {
            "disabled"
        }
    );
    let listener = TcpListener::bind(address).await?;
    let optimizer =
        RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver);
    let app = http::router_with_observation(optimizer, trasolve, observation);

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
