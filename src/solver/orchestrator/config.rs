use std::{collections::HashSet, time::Duration};

use crate::solver::{
    ClusteredSolverConfig, MatchingStrategyConfig, SimulatedAnnealingConfig, SolverError,
    DEFAULT_MAX_EXACT_CLUSTER_SIZE, EXACT_MAX_LOCATIONS,
};

const DEFAULT_MAX_CONCURRENCY: usize = 4;
const DEFAULT_STRATEGY_TIMEOUT: Duration = Duration::from_secs(30);

/// Configuration shared by all strategies in one orchestration run.
#[derive(Debug, Clone, PartialEq)]
pub struct SolverOrchestratorConfig {
    pub exact_limit: usize,
    pub max_cluster_size: usize,
    pub max_concurrency: usize,
    pub strategy_timeout: Duration,
    pub sa_config: SimulatedAnnealingConfig,
    pub sa_seeds: Vec<u64>,
    pub matching: MatchingStrategyConfig,
}

impl Default for SolverOrchestratorConfig {
    fn default() -> Self {
        Self {
            exact_limit: EXACT_MAX_LOCATIONS,
            max_cluster_size: DEFAULT_MAX_EXACT_CLUSTER_SIZE,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            strategy_timeout: DEFAULT_STRATEGY_TIMEOUT,
            sa_config: SimulatedAnnealingConfig::default(),
            sa_seeds: vec![42],
            matching: MatchingStrategyConfig::default(),
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
        self.sa_config.validate()?;
        MatchingStrategyConfig::new(self.matching.strategy, self.matching.bit_dp_threshold)?;
        Ok(self)
    }
}
