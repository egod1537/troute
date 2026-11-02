use std::{collections::HashSet, path::PathBuf, time::Duration};

use crate::solver::{
    ClusteredSolverConfig, MatchingStrategyConfig, PortfolioPolicyConfig, SimulatedAnnealingConfig,
    SolverError, SolverId, TelemetryMode, DEFAULT_MAX_EXACT_CLUSTER_SIZE, EXACT_MAX_LOCATIONS,
};

const DEFAULT_MAX_CONCURRENCY: usize = 4;
const DEFAULT_STRATEGY_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_TOTAL_BUDGET: Duration = Duration::from_secs(30);
const DEFAULT_SA_CHUNK: Duration = Duration::from_millis(250);
const DEFAULT_DEADLINE_SAFETY_MARGIN: Duration = Duration::from_millis(50);
const DEFAULT_TELEMETRY_MAX_EVENTS: usize = 10_000;

/// Configuration shared by all strategies in one orchestration run.
#[derive(Debug, Clone, PartialEq)]
pub struct SolverOrchestratorConfig {
    pub exact_limit: usize,
    pub max_cluster_size: usize,
    pub max_concurrency: usize,
    pub strategy_timeout: Duration,
    pub total_budget: Duration,
    pub sa_chunk: Duration,
    pub deadline_safety_margin: Duration,
    pub sa_config: SimulatedAnnealingConfig,
    pub sa_seeds: Vec<u64>,
    pub matching: MatchingStrategyConfig,
    pub enable_mip_scip: bool,
    pub mip_scip_time_limit: Duration,
    pub mip_scip_threads: usize,
    pub mip_scip_memory_mb: usize,
    pub enable_lkh3: bool,
    pub lkh3_executable: Option<PathBuf>,
    pub lkh3_time_limit: Duration,
    pub lkh3_seed: u64,
    pub compare_all: bool,
    pub incumbent_exchange: bool,
    pub portfolio: PortfolioPolicyConfig,
    pub telemetry_mode: TelemetryMode,
    pub telemetry_max_events: usize,
    pub max_external_processes: usize,
    pub memory_limit_bytes: Option<u64>,
    pub experiment_override_enabled: bool,
    pub enabled_solver_ids: Option<Vec<SolverId>>,
}

impl Default for SolverOrchestratorConfig {
    fn default() -> Self {
        Self {
            exact_limit: EXACT_MAX_LOCATIONS,
            max_cluster_size: DEFAULT_MAX_EXACT_CLUSTER_SIZE,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            strategy_timeout: DEFAULT_STRATEGY_TIMEOUT,
            total_budget: DEFAULT_TOTAL_BUDGET,
            sa_chunk: DEFAULT_SA_CHUNK,
            deadline_safety_margin: DEFAULT_DEADLINE_SAFETY_MARGIN,
            sa_config: SimulatedAnnealingConfig::default(),
            sa_seeds: vec![42],
            matching: MatchingStrategyConfig::default(),
            enable_mip_scip: false,
            mip_scip_time_limit: DEFAULT_STRATEGY_TIMEOUT,
            mip_scip_threads: 1,
            mip_scip_memory_mb: 1_024,
            enable_lkh3: false,
            lkh3_executable: None,
            lkh3_time_limit: DEFAULT_STRATEGY_TIMEOUT,
            lkh3_seed: 42,
            compare_all: false,
            incumbent_exchange: false,
            portfolio: PortfolioPolicyConfig::default(),
            telemetry_mode: TelemetryMode::Summary,
            telemetry_max_events: DEFAULT_TELEMETRY_MAX_EVENTS,
            max_external_processes: 1,
            memory_limit_bytes: None,
            experiment_override_enabled: false,
            enabled_solver_ids: None,
        }
    }
}

impl SolverOrchestratorConfig {
    pub fn validate(self) -> Result<Self, SolverError> {
        if self.exact_limit == 0 || self.exact_limit > EXACT_MAX_LOCATIONS {
            return Err(SolverError::InvalidConfiguration(format!(
                "orchestrator exact_limit must be between 1 and {EXACT_MAX_LOCATIONS}"
            )));
        }
        ClusteredSolverConfig::new(self.max_cluster_size)?;
        if self.max_concurrency == 0 {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator max_concurrency must be positive".to_owned(),
            ));
        }
        if self.strategy_timeout.is_zero() {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator strategy_timeout must be positive".to_owned(),
            ));
        }
        if self.total_budget.is_zero() {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator total_budget must be positive".to_owned(),
            ));
        }
        if self.sa_chunk.is_zero() {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator sa_chunk must be positive".to_owned(),
            ));
        }
        if self.deadline_safety_margin >= self.total_budget {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator deadline_safety_margin must be below total_budget".to_owned(),
            ));
        }
        if self.sa_seeds.is_empty() {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator sa_seeds must not be empty".to_owned(),
            ));
        }
        if self.sa_seeds.iter().copied().collect::<HashSet<_>>().len() != self.sa_seeds.len() {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator sa_seeds must be unique".to_owned(),
            ));
        }
        if self.mip_scip_time_limit.is_zero()
            || self.mip_scip_threads == 0
            || self.mip_scip_memory_mb == 0
        {
            return Err(SolverError::InvalidConfiguration(
                "SCIP time, thread, and memory limits must be positive".to_owned(),
            ));
        }
        if self.lkh3_time_limit.is_zero() {
            return Err(SolverError::InvalidConfiguration(
                "LKH-3 time limit must be positive".to_owned(),
            ));
        }
        if self.incumbent_exchange && !(self.enable_mip_scip && self.enable_lkh3) {
            return Err(SolverError::InvalidConfiguration(
                "incumbent exchange requires both SCIP and LKH-3".to_owned(),
            ));
        }
        if self.incumbent_exchange {
            return Err(SolverError::InvalidConfiguration(
                "incumbent exchange is experimental and remains unavailable until a real LKH-3 benchmark demonstrates a benefit; independent parallel strategies remain enabled"
                    .to_owned(),
            ));
        }
        self.portfolio
            .validate()
            .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))?;
        if self.telemetry_max_events == 0 || self.max_external_processes == 0 {
            return Err(SolverError::InvalidConfiguration(
                "telemetry_max_events and max_external_processes must be positive".to_owned(),
            ));
        }
        if self.enable_mip_scip && self.mip_scip_threads > self.max_concurrency {
            return Err(SolverError::InvalidConfiguration(
                "SOLVER_MIP_SCIP_THREADS must not exceed the logical CPU slot limit".to_owned(),
            ));
        }
        if let Some(enabled) = &self.enabled_solver_ids {
            let unique = enabled.iter().collect::<HashSet<_>>();
            if enabled.is_empty() || unique.len() != enabled.len() {
                return Err(SolverError::InvalidConfiguration(
                    "enabled solver IDs must be non-empty and unique".to_owned(),
                ));
            }
        }
        self.sa_config.validate()?;
        MatchingStrategyConfig::new(self.matching.strategy, self.matching.bit_dp_threshold)?;
        Ok(self)
    }
}
