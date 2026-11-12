use std::{
    fs,
    num::{NonZeroU16, NonZeroUsize},
    time::Duration,
};

use tempfile::tempdir;
use troute::{
    http::RouterRuntimeOptions, providers::tcache::TcacheRoutingConfig,
    routing::RouteProviderSelector, solver::SolverOrchestratorConfig, RemediationConfig,
};

use crate::config::{AppConfig, RoutingConfig, ServerConfig};

use super::ServerComponents;

#[tokio::test(flavor = "multi_thread")]
async fn initialization_propagates_storage_failures() {
    let root = tempdir().unwrap();
    let data_file = root.path().join("not-a-directory");
    fs::write(&data_file, b"occupied").unwrap();
    let provider_selector = RouteProviderSelector::from_raw_config_sources(
        "auto",
        None,
        None,
        None,
        Default::default(),
    )
    .unwrap();
    let config = AppConfig {
        server: ServerConfig {
            port: NonZeroU16::new(8080).unwrap(),
            data_dir: data_file,
            router: RouterRuntimeOptions::default(),
            validation_cpu_policy: troute::affinity::ValidationCpuPolicy::Off,
        },
        routing: RoutingConfig {
            tcache: TcacheRoutingConfig::from_parts(
                "http://127.0.0.1:1",
                Duration::from_millis(1),
                Duration::from_millis(1),
            )
            .unwrap(),
            provider_selector,
            pair_concurrency: NonZeroUsize::new(1).unwrap(),
            matrix_total_timeout: Duration::from_secs(1),
            default_euclidean_matrix: true,
        },
        solver: SolverOrchestratorConfig::default(),
        remediation: RemediationConfig::default(),
        startup_warnings: Vec::new(),
    };

    let error = match ServerComponents::initialize(config) {
        Ok(_) => panic!("bootstrap must propagate storage initialization failures"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("job storage I/O failed"));
}
