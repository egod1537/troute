use std::{error::Error, net::SocketAddr, sync::Arc};

use axum::Router;
use troute::{
    http, logging::logger, matrix::MatrixService, solver::SolverOrchestrator,
    RouteOptimizationService,
};

use crate::config::{AppConfig, ServerConfig};

use super::{routing::RoutingRuntime, startup::StartupSummary, storage::StorageRuntime};

/// Dependencies produced during synchronous startup before the listener is
/// bound. This is the manual DI composition root for the HTTP server.
pub(super) struct ServerComponents {
    bind_addr: SocketAddr,
    router: Router,
}

impl ServerComponents {
    pub(super) fn initialize(config: AppConfig) -> Result<Self, Box<dyn Error>> {
        let AppConfig {
            server,
            routing,
            solver,
            remediation,
            startup_warnings,
        } = config;
        for warning in startup_warnings {
            logger().startup_warning(&warning);
        }

        let ServerConfig {
            port,
            data_dir,
            router: router_options,
        } = server;
        let routing_runtime = RoutingRuntime::initialize(routing)?;
        let solver = SolverOrchestrator::new(solver)?;
        let storage_runtime = StorageRuntime::initialize(&data_dir)?;
        let bind_addr = SocketAddr::from(([0, 0, 0, 0], port.get()));

        StartupSummary::new(
            bind_addr,
            &data_dir,
            &storage_runtime,
            solver.config(),
            &routing_runtime,
            remediation,
        )
        .log();

        let matrix_service = Arc::new(MatrixService::new(routing_runtime.into_provider()));
        let optimizer = RouteOptimizationService::with_matrix_service(matrix_service, solver)
            .with_remediation_config(remediation);
        let router = http::router_with_storage_runtime_options(
            optimizer,
            Some(storage_runtime.observation_recorder()),
            Some(storage_runtime.job_store()),
            router_options,
        );

        Ok(Self { bind_addr, router })
    }

    pub(super) fn bind_addr(&self) -> SocketAddr {
        self.bind_addr
    }

    pub(super) fn into_router(self) -> Router {
        self.router
    }
}

#[cfg(test)]
mod tests;
