pub(crate) mod env;

use std::{
    collections::BTreeMap,
    error::Error,
    num::{NonZeroU16, NonZeroUsize},
    path::PathBuf,
    time::Duration,
};

use env::{EnvReader, EnvSource, ProcessEnv};
use troute::{
    http::RouterRuntimeOptions,
    providers::tcache::TcacheRoutingConfig,
    routing::{RouteProviderSelector, DEFAULT_MATRIX_TOTAL_TIMEOUT_MS, DEFAULT_PAIR_CONCURRENCY},
    solver::{
        MatchingStrategyConfig, PortfolioPolicy, PortfolioPolicyConfig, SolverError,
        SolverOrchestratorConfig, TelemetryMode, EXACT_MAX_LOCATIONS, MAX_EXACT_CLUSTER_SIZE,
    },
    RemediationConfig,
};

#[derive(Debug, Clone)]
pub(crate) struct AppConfig {
    pub(crate) server: ServerConfig,
    pub(crate) routing: RoutingConfig,
    pub(crate) solver: SolverOrchestratorConfig,
    pub(crate) remediation: RemediationConfig,
    pub(crate) startup_warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerConfig {
    pub(crate) port: NonZeroU16,
    pub(crate) data_dir: PathBuf,
    pub(crate) router: RouterRuntimeOptions,
}

#[derive(Debug, Clone)]
pub(crate) struct RoutingConfig {
    pub(crate) tcache: TcacheRoutingConfig,
    pub(crate) provider_selector: RouteProviderSelector,
    pub(crate) pair_concurrency: NonZeroUsize,
    pub(crate) matrix_total_timeout: Duration,
    pub(crate) default_euclidean_matrix: bool,
}

impl AppConfig {
    pub(crate) fn from_env() -> Result<Self, Box<dyn Error>> {
        Self::from_source(&ProcessEnv)
    }

    fn from_source<S: EnvSource>(source: &S) -> Result<Self, Box<dyn Error>> {
        let reader = EnvReader::new(source);
        let mut startup_warnings = Vec::new();
        let server = ServerConfig::from_source(&reader, &mut startup_warnings)?;
        let routing = RoutingConfig::from_source(&reader, &mut startup_warnings)?;
        let solver = read_solver_config(&reader)?.validate()?;
        let remediation = read_remediation_config(&reader)?;

        Ok(Self {
            server,
            routing,
            solver,
            remediation,
            startup_warnings,
        })
    }
}

impl ServerConfig {
    fn from_source<S: EnvSource>(
        reader: &EnvReader<'_, S>,
        warnings: &mut Vec<String>,
    ) -> Result<Self, String> {
        let port = reader.read_bounded_size("TROUTE_PORT", 8080, usize::from(u16::MAX))?;
        let port = NonZeroU16::new(u16::try_from(port).expect("validated port must fit u16"))
            .expect("validated port must be positive");
        let data_dir = reader.read_nonempty_path("TROUTE_DATA_DIR", ".local/troute")?;
        let defaults = RouterRuntimeOptions::default();
        let max_concurrent_jobs = Some(read_warned_job_limit(
            reader,
            "TROUTE_MAX_CONCURRENT_JOBS",
            defaults
                .max_concurrent_jobs
                .expect("the default job concurrency is bounded"),
            warnings,
        ));
        let max_outstanding_jobs = Some(read_warned_positive_limit(
            reader,
            "TROUTE_MAX_OUTSTANDING_JOBS",
            defaults
                .max_outstanding_jobs
                .expect("the default outstanding job count is bounded"),
            warnings,
        ));
        let max_concurrent_matrix_previews = read_warned_positive_limit(
            reader,
            "TROUTE_MAX_CONCURRENT_MATRIX_PREVIEWS",
            defaults.max_concurrent_matrix_previews,
            warnings,
        );
        let max_routing_pairs =
            read_warned_routing_pair_limit(reader, defaults.max_routing_pairs, warnings);

        Ok(Self {
            port,
            data_dir,
            router: RouterRuntimeOptions {
                max_concurrent_jobs,
                max_outstanding_jobs,
                max_concurrent_matrix_previews,
                max_routing_pairs,
                sse_heartbeat_interval: defaults.sse_heartbeat_interval,
            },
        })
    }
}

impl RoutingConfig {
    fn from_source<S: EnvSource>(
        reader: &EnvReader<'_, S>,
        warnings: &mut Vec<String>,
    ) -> Result<Self, Box<dyn Error>> {
        let tcache = read_tcache_config(reader)
            .map_err(|error| format!("invalid tcache configuration: {error}"))?;

        let route_provider = reader.read_string_or_default("ROUTE_PROVIDER", "auto".to_owned());
        let policy_json = reader.lookup("ROUTE_PROVIDER_POLICY_JSON").ok().flatten();
        let legacy_provider = reader.lookup("JAPAN_TRANSIT_PROVIDER").ok().flatten();
        let override_enabled = reader
            .lookup("ROUTE_PROVIDER_OVERRIDE_ENABLED")
            .map_err(|error| error.to_string())?;
        let provider_selector = RouteProviderSelector::from_raw_config_sources(
            &route_provider,
            policy_json.as_deref(),
            legacy_provider.as_deref(),
            override_enabled.as_deref(),
            Default::default(),
        )
        .map_err(|error| format!("invalid route provider policy: {error}"))?;
        if reader.has_unicode_value("JAPAN_TRANSIT_PROVIDER") {
            warnings.push(
                "JAPAN_TRANSIT_PROVIDER is deprecated; use ROUTE_PROVIDER_POLICY_JSON".to_owned(),
            );
        }

        let pair_concurrency =
            reader.read_bounded_size("TCACHE_PAIR_CONCURRENCY", DEFAULT_PAIR_CONCURRENCY, 256)?;
        let pair_concurrency = NonZeroUsize::new(pair_concurrency)
            .expect("validated pair concurrency must be positive");
        let matrix_total_timeout_ms = reader.read_bounded_size(
            "TCACHE_MATRIX_TOTAL_TIMEOUT_MS",
            usize::try_from(DEFAULT_MATRIX_TOTAL_TIMEOUT_MS)
                .expect("default matrix timeout must fit usize"),
            86_400_000,
        )?;
        let default_euclidean_matrix = reader.read_bool("TROUTE_DEFAULT_EUCLIDEAN_MATRIX", true)?;

        Ok(Self {
            tcache,
            provider_selector,
            pair_concurrency,
            matrix_total_timeout: Duration::from_millis(
                u64::try_from(matrix_total_timeout_ms).expect("matrix timeout must fit u64"),
            ),
            default_euclidean_matrix,
        })
    }
}

fn read_solver_config<S: EnvSource>(
    reader: &EnvReader<'_, S>,
) -> Result<SolverOrchestratorConfig, Box<dyn Error>> {
    let defaults = SolverOrchestratorConfig::default();
    let portfolio_defaults = defaults.portfolio.clone();
    let exact_limit = reader.read_bounded_size(
        "TROUTE_EXACT_LIMIT",
        defaults.exact_limit,
        EXACT_MAX_LOCATIONS,
    )?;
    let max_cluster_size = reader.read_bounded_size(
        "TROUTE_MAX_EXACT_CLUSTER_SIZE",
        defaults.max_cluster_size,
        MAX_EXACT_CLUSTER_SIZE,
    )?;
    let max_concurrency_name = if reader.is_present("SOLVER_CPU_SLOTS") {
        "SOLVER_CPU_SLOTS"
    } else {
        "SOLVER_MAX_CONCURRENCY"
    };
    let max_concurrency =
        reader.read_bounded_size(max_concurrency_name, defaults.max_concurrency, 256)?;
    let strategy_timeout_ms = reader.read_positive_u64(
        "SOLVER_STRATEGY_TIMEOUT_MS",
        duration_millis(defaults.strategy_timeout),
    )?;
    let total_budget_ms = reader.read_positive_u64(
        "TROUTE_SOLVER_TOTAL_BUDGET_MS",
        duration_millis(defaults.total_budget),
    )?;
    let sa_chunk_ms =
        reader.read_positive_u64("SOLVER_SA_CHUNK_MS", duration_millis(defaults.sa_chunk))?;
    let deadline_safety_ms = reader.read_positive_u64(
        "SOLVER_DEADLINE_SAFETY_MS",
        duration_millis(defaults.deadline_safety_margin),
    )?;
    let sa_seeds = reader.read_u64_list("SOLVER_SA_SEEDS", &defaults.sa_seeds)?;
    let enable_mip_scip = reader.read_bool("SOLVER_ENABLE_MIP_SCIP", defaults.enable_mip_scip)?;
    let mip_scip_time_limit_ms = reader.read_positive_u64(
        "SOLVER_MIP_SCIP_TIMEOUT_MS",
        duration_millis(defaults.mip_scip_time_limit),
    )?;
    let mip_scip_threads =
        reader.read_bounded_size("SOLVER_MIP_SCIP_THREADS", defaults.mip_scip_threads, 256)?;
    let mip_scip_memory_mb = reader.read_bounded_size(
        "SOLVER_MIP_SCIP_MEMORY_MB",
        defaults.mip_scip_memory_mb,
        1_000_000,
    )?;
    let enable_lkh3 = reader.read_bool("SOLVER_ENABLE_LKH3", defaults.enable_lkh3)?;
    let lkh3_executable = reader.read_optional_path("LKH3_EXECUTABLE");
    let lkh3_time_limit_ms = reader.read_positive_u64(
        "SOLVER_LKH3_TIMEOUT_MS",
        duration_millis(defaults.lkh3_time_limit),
    )?;
    let lkh3_seed = reader.read_positive_u64("SOLVER_LKH3_SEED", defaults.lkh3_seed)?;
    let compare_all = reader.read_bool("SOLVER_COMPARE_ALL", defaults.compare_all)?;
    let incumbent_exchange =
        reader.read_bool("SOLVER_INCUMBENT_EXCHANGE", defaults.incumbent_exchange)?;
    let portfolio_policy = reader
        .read_string_or_default(
            "SOLVER_PORTFOLIO_POLICY",
            portfolio_defaults.policy.to_string(),
        )
        .parse::<PortfolioPolicy>()
        .map_err(|error| error.to_string())?;
    let policy_seed = reader.read_positive_u64("SOLVER_POLICY_SEED", portfolio_defaults.seed)?;
    let policy_min_quantum_ms =
        reader.read_positive_u64("SOLVER_POLICY_MIN_QUANTUM_MS", sa_chunk_ms)?;
    let racing_warmup_runs = reader.read_bounded_size(
        "SOLVER_RACING_WARMUP_RUNS",
        portfolio_defaults.racing_warmup_runs,
        10_000,
    )?;
    let ucb_exploration = reader.read_f64(
        "SOLVER_UCB_EXPLORATION",
        portfolio_defaults.ucb_exploration,
        0.0,
        f64::MAX,
    )?;
    let min_exploration_ratio = reader.read_f64(
        "SOLVER_MIN_EXPLORATION_RATIO",
        portfolio_defaults.min_exploration_ratio,
        0.0,
        1.0,
    )?;
    let max_starvation_runs = reader.read_positive_u64(
        "SOLVER_MAX_STARVATION_RUNS",
        portfolio_defaults.max_starvation_runs,
    )?;
    let policy_weights = read_policy_weights(reader)?;
    let telemetry_mode = read_telemetry_mode(reader, defaults.telemetry_mode)?;
    let telemetry_max_events = reader.read_bounded_size(
        "SOLVER_TELEMETRY_MAX_EVENTS",
        defaults.telemetry_max_events,
        100_000,
    )?;
    let max_external_processes = reader.read_bounded_size(
        "SOLVER_MAX_EXTERNAL_PROCESSES",
        defaults.max_external_processes,
        64,
    )?;
    let memory_limit_bytes = read_memory_limit(reader)?;
    let experiment_override_enabled = reader.read_bool(
        "TROUTE_PORTFOLIO_EXPERIMENT_ENABLED",
        defaults.experiment_override_enabled,
    )?;

    Ok(SolverOrchestratorConfig {
        exact_limit,
        max_cluster_size,
        max_concurrency,
        strategy_timeout: Duration::from_millis(strategy_timeout_ms),
        total_budget: Duration::from_millis(total_budget_ms),
        sa_chunk: Duration::from_millis(sa_chunk_ms),
        deadline_safety_margin: Duration::from_millis(deadline_safety_ms),
        sa_seeds,
        matching: read_matching_config(reader)?,
        enable_mip_scip,
        mip_scip_time_limit: Duration::from_millis(mip_scip_time_limit_ms),
        mip_scip_threads,
        mip_scip_memory_mb,
        enable_lkh3,
        lkh3_executable,
        lkh3_time_limit: Duration::from_millis(lkh3_time_limit_ms),
        lkh3_seed,
        compare_all,
        incumbent_exchange,
        portfolio: PortfolioPolicyConfig {
            policy: portfolio_policy,
            seed: policy_seed,
            min_quantum_ms: policy_min_quantum_ms,
            racing_warmup_runs,
            ucb_exploration,
            min_exploration_ratio,
            max_starvation_runs,
            weights: policy_weights,
            ..portfolio_defaults
        },
        telemetry_mode,
        telemetry_max_events,
        max_external_processes,
        memory_limit_bytes,
        experiment_override_enabled,
        ..defaults
    })
}

fn read_matching_config<S: EnvSource>(
    reader: &EnvReader<'_, S>,
) -> Result<MatchingStrategyConfig, SolverError> {
    let strategy = reader.lookup("MATCHING_STRATEGY").map_err(|error| {
        SolverError::InvalidConfiguration(format!("MATCHING_STRATEGY is invalid: {error}"))
    })?;
    let threshold = reader
        .lookup("MATCHING_BIT_DP_THRESHOLD")
        .map_err(|error| {
            SolverError::InvalidConfiguration(format!(
                "MATCHING_BIT_DP_THRESHOLD is invalid: {error}"
            ))
        })?;
    MatchingStrategyConfig::from_config_sources(strategy.as_deref(), threshold.as_deref())
}

fn read_tcache_config<S: EnvSource>(
    reader: &EnvReader<'_, S>,
) -> Result<TcacheRoutingConfig, String> {
    let base_url = reader
        .lookup("TCACHE_BASE_URL")
        .ok()
        .flatten()
        .ok_or_else(|| "TCACHE_BASE_URL is required".to_owned())?;
    let poll_interval = reader
        .lookup("TCACHE_POLL_INTERVAL_MS")
        .map_err(|error| error.to_string())?;
    let legacy_poll_interval = reader
        .lookup("TCACHE_MATRIX_POLL_INTERVAL_MS")
        .map_err(|error| error.to_string())?;
    let timeout = reader
        .lookup("TCACHE_REQUEST_TIMEOUT_MS")
        .map_err(|error| error.to_string())?;
    let legacy_timeout = reader
        .lookup("TCACHE_MATRIX_TIMEOUT_MS")
        .map_err(|error| error.to_string())?;
    TcacheRoutingConfig::from_config_sources(
        &base_url,
        poll_interval.as_deref(),
        legacy_poll_interval.as_deref(),
        timeout.as_deref(),
        legacy_timeout.as_deref(),
    )
}

fn read_remediation_config<S: EnvSource>(
    reader: &EnvReader<'_, S>,
) -> Result<RemediationConfig, String> {
    let defaults = RemediationConfig::default();
    let budget_ms = reader.read_positive_u64(
        "TROUTE_REMEDIATION_BUDGET_MS",
        duration_millis(defaults.budget),
    )?;
    let max_remove_probes = reader.read_bounded_size(
        "TROUTE_REMEDIATION_MAX_REMOVE_PROBES",
        defaults.max_remove_probes,
        500,
    )?;
    let max_suggestions = reader.read_bounded_size(
        "TROUTE_MAX_FAILURE_SUGGESTIONS",
        defaults.max_suggestions,
        100,
    )?;
    Ok(RemediationConfig {
        budget: Duration::from_millis(budget_ms),
        max_remove_probes,
        max_suggestions,
    })
}

fn read_policy_weights<S: EnvSource>(
    reader: &EnvReader<'_, S>,
) -> Result<BTreeMap<String, f64>, String> {
    match reader.read_optional_string("SOLVER_POLICY_WEIGHTS_JSON")? {
        Some(value) if !value.trim().is_empty() => serde_json::from_str(&value)
            .map_err(|error| format!("SOLVER_POLICY_WEIGHTS_JSON must be a JSON object: {error}")),
        Some(_) | None => Ok(BTreeMap::new()),
    }
}

fn read_telemetry_mode<S: EnvSource>(
    reader: &EnvReader<'_, S>,
    default: TelemetryMode,
) -> Result<TelemetryMode, String> {
    let default = match default {
        TelemetryMode::Off => "OFF",
        TelemetryMode::Summary => "SUMMARY",
        TelemetryMode::Detailed => "DETAILED",
    };
    match reader
        .read_string_or_default("SOLVER_TELEMETRY_MODE", default.to_owned())
        .to_ascii_uppercase()
        .as_str()
    {
        "OFF" => Ok(TelemetryMode::Off),
        "SUMMARY" => Ok(TelemetryMode::Summary),
        "DETAILED" => Ok(TelemetryMode::Detailed),
        _ => Err("SOLVER_TELEMETRY_MODE must be OFF, SUMMARY, or DETAILED".to_owned()),
    }
}

fn read_memory_limit<S: EnvSource>(reader: &EnvReader<'_, S>) -> Result<Option<u64>, String> {
    reader
        .read_optional_string("SOLVER_MEMORY_LIMIT_MB")?
        .map(|value| {
            value
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| "SOLVER_MEMORY_LIMIT_MB must be a positive integer".to_owned())
                .map(|value| value.saturating_mul(1024 * 1024))
        })
        .transpose()
}

fn read_warned_job_limit<S: EnvSource>(
    reader: &EnvReader<'_, S>,
    name: &str,
    default: usize,
    warnings: &mut Vec<String>,
) -> usize {
    match reader.lookup(name) {
        Ok(Some(value)) => match value.parse::<usize>() {
            Ok(limit) if limit > 0 => limit,
            _ => {
                warnings.push(format!("invalid {name}={value:?}; using default {default}"));
                default
            }
        },
        Ok(None) => default,
        Err(error) => {
            warnings.push(format!(
                "cannot read {name}: {error}; using default {default}"
            ));
            default
        }
    }
}

fn read_warned_positive_limit<S: EnvSource>(
    reader: &EnvReader<'_, S>,
    name: &str,
    default: usize,
    warnings: &mut Vec<String>,
) -> usize {
    match reader.lookup(name) {
        Ok(Some(value)) => value
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .unwrap_or_else(|| {
                warnings.push(format!("invalid {name}={value:?}; using default {default}"));
                default
            }),
        Ok(None) | Err(_) => default,
    }
}

fn read_warned_routing_pair_limit<S: EnvSource>(
    reader: &EnvReader<'_, S>,
    default: usize,
    warnings: &mut Vec<String>,
) -> usize {
    const NAME: &str = "TROUTE_MAX_ROUTING_PAIRS";
    match reader.lookup(NAME) {
        Ok(Some(value)) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| *limit > 0)
            .unwrap_or_else(|| {
                warnings.push(format!("invalid {NAME}={value:?}; using default {default}"));
                default
            }),
        Ok(None) | Err(_) => default,
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
