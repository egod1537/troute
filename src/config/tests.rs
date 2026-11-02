use std::{
    collections::BTreeMap,
    env::VarError,
    ffi::OsString,
    path::{Path, PathBuf},
};

use super::{env::EnvSource, AppConfig};

struct ErrorSource {
    values: BTreeMap<String, String>,
    error_name: &'static str,
}

impl EnvSource for ErrorSource {
    fn var(&self, name: &str) -> Result<Option<String>, VarError> {
        if name == self.error_name {
            Err(VarError::NotUnicode(OsString::from("non-unicode-value")))
        } else {
            Ok(self.values.get(name).cloned())
        }
    }

    fn var_os(&self, name: &str) -> Option<OsString> {
        if name == self.error_name {
            Some(OsString::from("non-unicode-value"))
        } else {
            self.values.get(name).map(OsString::from)
        }
    }
}

fn source(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn app_config_loads_defaults_without_mutating_process_environment() {
    let missing = source(&[]);
    let without_tcache = AppConfig::from_source(&missing).unwrap();
    assert!(without_tcache.routing.tcache.is_none());

    let values = source(&[("TCACHE_BASE_URL", "http://localhost:8081")]);
    let defaults = AppConfig::from_source(&values).unwrap();
    assert!(defaults.routing.tcache.is_some());
    assert_eq!(defaults.server.port.get(), 8080);
    assert_eq!(defaults.server.data_dir, PathBuf::from(".local/troute"));
    assert_eq!(
        defaults.routing.pair_concurrency.get(),
        troute::routing::DEFAULT_PAIR_CONCURRENCY
    );
    assert_eq!(
        defaults.routing.matrix_total_timeout.as_millis(),
        u128::from(troute::routing::DEFAULT_MATRIX_TOTAL_TIMEOUT_MS)
    );
    assert_eq!(
        defaults.solver,
        troute::solver::SolverOrchestratorConfig::default()
    );
    assert_eq!(defaults.remediation, troute::RemediationConfig::default());
    assert_eq!(
        defaults.server.router,
        troute::http::RouterRuntimeOptions::default()
    );
    assert!(defaults.startup_warnings.is_empty());
}

#[test]
fn app_config_loads_typed_values_and_preserves_alias_priority() {
    let values = source(&[
        ("TCACHE_BASE_URL", "http://localhost:8081"),
        ("TCACHE_POLL_INTERVAL_MS", "400"),
        ("TCACHE_MATRIX_POLL_INTERVAL_MS", "500"),
        ("TROUTE_PORT", "9090"),
        ("TROUTE_DATA_DIR", "test-data"),
        ("TCACHE_PAIR_CONCURRENCY", "12"),
        ("TCACHE_MATRIX_TOTAL_TIMEOUT_MS", "1234"),
        ("SOLVER_MAX_CONCURRENCY", "2"),
        ("SOLVER_CPU_SLOTS", "6"),
        ("SOLVER_SA_SEEDS", "1, 2, 3"),
        ("SOLVER_ENABLE_MIP_SCIP", "true"),
        ("SOLVER_MIP_SCIP_THREADS", "2"),
        ("SOLVER_UCB_EXPLORATION", "1.5"),
        ("SOLVER_MIN_EXPLORATION_RATIO", "0.25"),
        ("SOLVER_MEMORY_LIMIT_MB", "2"),
        ("TROUTE_REMEDIATION_BUDGET_MS", "1500"),
        ("TROUTE_REMEDIATION_MAX_REMOVE_PROBES", "20"),
        ("TROUTE_MAX_FAILURE_SUGGESTIONS", "7"),
        ("TROUTE_MAX_CONCURRENT_JOBS", "3"),
        ("TROUTE_MAX_OUTSTANDING_JOBS", "40"),
        ("TROUTE_MAX_CONCURRENT_MATRIX_PREVIEWS", "5"),
        ("TROUTE_MAX_ROUTING_PAIRS", "800"),
        ("ROUTE_PROVIDER_OVERRIDE_ENABLED", "1"),
    ]);
    let configured = AppConfig::from_source(&values).unwrap();
    assert_eq!(configured.server.port.get(), 9090);
    assert_eq!(configured.server.data_dir, PathBuf::from("test-data"));
    assert_eq!(configured.server.router.max_concurrent_jobs, Some(3));
    assert_eq!(configured.server.router.max_outstanding_jobs, Some(40));
    assert_eq!(configured.server.router.max_concurrent_matrix_previews, 5);
    assert_eq!(configured.server.router.max_routing_pairs, 800);
    assert_eq!(
        configured
            .routing
            .tcache
            .as_ref()
            .unwrap()
            .poll_interval
            .as_millis(),
        400
    );
    assert_eq!(configured.routing.pair_concurrency.get(), 12);
    assert_eq!(configured.routing.matrix_total_timeout.as_millis(), 1234);
    assert!(
        configured
            .routing
            .provider_selector
            .diagnostics()
            .override_enabled
    );
    assert_eq!(configured.solver.max_concurrency, 6);
    assert_eq!(configured.solver.sa_seeds, vec![1, 2, 3]);
    assert!(configured.solver.enable_mip_scip);
    assert_eq!(configured.solver.mip_scip_threads, 2);
    assert_eq!(configured.solver.memory_limit_bytes, Some(2_097_152));
    assert_eq!(configured.remediation.budget.as_millis(), 1500);
    assert_eq!(configured.remediation.max_remove_probes, 20);
    assert_eq!(configured.remediation.max_suggestions, 7);
}

#[test]
fn app_config_rejects_invalid_type_range_list_and_cross_field_values() {
    for (name, value, expected) in [
            (
                "TROUTE_PORT",
                "invalid",
                "TROUTE_PORT must be an integer between 1 and 65535",
            ),
            (
                "TCACHE_PAIR_CONCURRENCY",
                "257",
                "TCACHE_PAIR_CONCURRENCY must be an integer between 1 and 256",
            ),
            (
                "SOLVER_SA_SEEDS",
                "1,not-a-seed",
                "SOLVER_SA_SEEDS must be a comma-separated list of u64 seeds",
            ),
            ("TROUTE_DATA_DIR", "", "TROUTE_DATA_DIR must not be empty"),
            (
                "ROUTE_PROVIDER_OVERRIDE_ENABLED",
                "yes",
                "invalid route provider policy: ROUTE_PROVIDER_OVERRIDE_ENABLED must be true or false, got \"yes\"",
            ),
        ] {
            let values = source(&[("TCACHE_BASE_URL", "http://localhost:8081"), (name, value)]);
            assert_eq!(
                AppConfig::from_source(&values).unwrap_err().to_string(),
                expected
            );
        }

    let invalid_budget = source(&[
        ("TCACHE_BASE_URL", "http://localhost:8081"),
        ("TROUTE_SOLVER_TOTAL_BUDGET_MS", "50"),
        ("SOLVER_DEADLINE_SAFETY_MS", "50"),
    ]);
    assert!(AppConfig::from_source(&invalid_budget)
        .unwrap_err()
        .to_string()
        .contains("deadline_safety_margin must be below total_budget"));

    let duplicate_seeds = source(&[
        ("TCACHE_BASE_URL", "http://localhost:8081"),
        ("SOLVER_SA_SEEDS", "7,7"),
    ]);
    assert!(AppConfig::from_source(&duplicate_seeds)
        .unwrap_err()
        .to_string()
        .contains("sa_seeds must be unique"));
}

#[test]
fn compatibility_limits_warn_and_fall_back_instead_of_failing_startup() {
    let values = source(&[
        ("TCACHE_BASE_URL", "http://localhost:8081"),
        ("TROUTE_MAX_CONCURRENT_JOBS", "0"),
        ("TROUTE_MAX_OUTSTANDING_JOBS", "bad"),
        ("TROUTE_MAX_CONCURRENT_MATRIX_PREVIEWS", "0"),
        ("TROUTE_MAX_ROUTING_PAIRS", "bad"),
        ("JAPAN_TRANSIT_PROVIDER", "google"),
    ]);
    let configured = AppConfig::from_source(&values).unwrap();
    assert_eq!(
        configured.server.router,
        troute::http::RouterRuntimeOptions::default()
    );
    assert_eq!(configured.startup_warnings.len(), 5);
    assert!(configured
        .startup_warnings
        .iter()
        .any(|warning| warning.contains("JAPAN_TRANSIT_PROVIDER is deprecated")));
}

#[test]
fn invalid_new_tcache_alias_never_falls_back_to_valid_legacy_value() {
    let values = source(&[
        ("TCACHE_BASE_URL", "http://localhost:8081"),
        ("TCACHE_POLL_INTERVAL_MS", "bad"),
        ("TCACHE_MATRIX_POLL_INTERVAL_MS", "500"),
    ]);
    assert_eq!(
        AppConfig::from_source(&values).unwrap_err().to_string(),
        "invalid tcache configuration: TCACHE_POLL_INTERVAL_MS must be a positive integer"
    );
}

#[test]
fn non_unicode_values_follow_each_settings_existing_error_policy() {
    let values = source(&[("TCACHE_BASE_URL", "http://localhost:8081")]);
    let invalid_port = ErrorSource {
        values: values.clone(),
        error_name: "TROUTE_PORT",
    };
    assert!(AppConfig::from_source(&invalid_port)
        .unwrap_err()
        .to_string()
        .contains("not valid unicode"));

    let invalid_base_url = ErrorSource {
        values: BTreeMap::new(),
        error_name: "TCACHE_BASE_URL",
    };
    assert!(AppConfig::from_source(&invalid_base_url)
        .unwrap_err()
        .to_string()
        .starts_with("invalid tcache configuration:"));

    let warned_limit = ErrorSource {
        values: values.clone(),
        error_name: "TROUTE_MAX_CONCURRENT_JOBS",
    };
    let configured = AppConfig::from_source(&warned_limit).unwrap();
    assert_eq!(configured.server.router.max_concurrent_jobs, Some(4));
    assert!(configured.startup_warnings[0].starts_with("cannot read TROUTE_MAX_CONCURRENT_JOBS:"));

    let os_path = ErrorSource {
        values,
        error_name: "LKH3_EXECUTABLE",
    };
    let configured = AppConfig::from_source(&os_path).unwrap();
    assert_eq!(
        configured.solver.lkh3_executable.as_deref(),
        Some(Path::new("non-unicode-value"))
    );
}

#[test]
fn blank_tcache_base_url_disables_tcache() {
    let configured = AppConfig::from_source(&source(&[("TCACHE_BASE_URL", "  ")])).unwrap();
    assert!(configured.routing.tcache.is_none());
}
