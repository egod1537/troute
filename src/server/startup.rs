use std::{net::SocketAddr, path::Path};

use troute::{
    logging::{logger, ServerStartupLog},
    solver::SolverOrchestratorConfig,
    RemediationConfig,
};

use super::{routing::RoutingRuntime, storage::StorageRuntime};

/// Read-only startup diagnostics assembled from initialized runtime objects.
pub(super) struct StartupSummary<'a> {
    bind_addr: SocketAddr,
    data_dir: &'a Path,
    storage: &'a StorageRuntime,
    solver: &'a SolverOrchestratorConfig,
    routing: &'a RoutingRuntime,
    remediation: RemediationConfig,
}

impl<'a> StartupSummary<'a> {
    pub(super) fn new(
        bind_addr: SocketAddr,
        data_dir: &'a Path,
        storage: &'a StorageRuntime,
        solver: &'a SolverOrchestratorConfig,
        routing: &'a RoutingRuntime,
        remediation: RemediationConfig,
    ) -> Self {
        Self {
            bind_addr,
            data_dir,
            storage,
            solver,
            routing,
            remediation,
        }
    }

    pub(super) fn log(self) {
        let provider = self.routing.provider_diagnostics();
        logger().server_starting(ServerStartupLog {
            bind_addr: self.bind_addr,
            data_dir: self.data_dir,
            recovered_jobs: self.storage.recovered_jobs(),
            solver: self.solver,
            provider,
            pair_concurrency: self.routing.pair_concurrency(),
            matrix_total_timeout: self.routing.matrix_total_timeout(),
            remediation: self.remediation,
        });
    }
}
