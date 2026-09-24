mod bitdp;
mod blossom;
mod traits;

use std::{env, fmt, str::FromStr};

use crate::solver::SolverError;

pub use bitdp::*;
pub use blossom::*;
pub use traits::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchingStrategyChoice {
    Auto,
    BitDp,
    Blossom,
}

impl FromStr for MatchingStrategyChoice {
    type Err = SolverError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "bitdp" => Ok(Self::BitDp),
            "blossom" => Ok(Self::Blossom),
            _ => Err(SolverError::InvalidConfiguration(format!(
                "MATCHING_STRATEGY must be auto, bitdp, or blossom (actual {value})"
            ))),
        }
    }
}

impl fmt::Display for MatchingStrategyChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::BitDp => "bitdp",
            Self::Blossom => "blossom",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchingStrategyConfig {
    pub strategy: MatchingStrategyChoice,
    pub bit_dp_threshold: usize,
}

impl Default for MatchingStrategyConfig {
    fn default() -> Self {
        Self {
            strategy: MatchingStrategyChoice::Auto,
            bit_dp_threshold: MAX_BIT_DP_MATCHING_VERTICES,
        }
    }
}

impl MatchingStrategyConfig {
    pub fn new(
        strategy: MatchingStrategyChoice,
        bit_dp_threshold: usize,
    ) -> Result<Self, SolverError> {
        if bit_dp_threshold > MAX_BIT_DP_MATCHING_VERTICES {
            return Err(SolverError::InvalidConfiguration(format!(
                "MATCHING_BIT_DP_THRESHOLD must be between 0 and {MAX_BIT_DP_MATCHING_VERTICES}"
            )));
        }
        Ok(Self {
            strategy,
            bit_dp_threshold,
        })
    }

    /// Reads benchmark/debug overrides. Solvers still receive the resulting
    /// value explicitly and do not perform environment lookup while solving.
    pub fn from_env() -> Result<Self, SolverError> {
        let strategy = match env::var("MATCHING_STRATEGY") {
            Ok(value) => value.parse()?,
            Err(env::VarError::NotPresent) => MatchingStrategyChoice::Auto,
            Err(error) => {
                return Err(SolverError::InvalidConfiguration(format!(
                    "MATCHING_STRATEGY is invalid: {error}"
                )))
            }
        };
        let threshold = match env::var("MATCHING_BIT_DP_THRESHOLD") {
            Ok(value) => value.parse::<usize>().map_err(|_| {
                SolverError::InvalidConfiguration(format!(
                    "MATCHING_BIT_DP_THRESHOLD must be an integer between 0 and {MAX_BIT_DP_MATCHING_VERTICES}"
                ))
            })?,
            Err(env::VarError::NotPresent) => MAX_BIT_DP_MATCHING_VERTICES,
            Err(error) => {
                return Err(SolverError::InvalidConfiguration(format!(
                    "MATCHING_BIT_DP_THRESHOLD is invalid: {error}"
                )))
            }
        };
        Self::new(strategy, threshold)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoPerfectMatching {
    config: MatchingStrategyConfig,
    bit_dp: BitDpPerfectMatching,
    blossom: BlossomPerfectMatching,
}

impl Default for AutoPerfectMatching {
    fn default() -> Self {
        Self::new(MatchingStrategyConfig::default())
            .expect("the default matching configuration is valid")
    }
}

impl AutoPerfectMatching {
    pub fn new(config: MatchingStrategyConfig) -> Result<Self, SolverError> {
        let config = MatchingStrategyConfig::new(config.strategy, config.bit_dp_threshold)?;
        Ok(Self {
            config,
            bit_dp: BitDpPerfectMatching::default(),
            blossom: BlossomPerfectMatching,
        })
    }

    pub fn config(&self) -> MatchingStrategyConfig {
        self.config
    }

    pub fn selected_strategy(&self, vertex_count: usize) -> MatchingStrategyChoice {
        match self.config.strategy {
            MatchingStrategyChoice::Auto if vertex_count <= self.config.bit_dp_threshold => {
                MatchingStrategyChoice::BitDp
            }
            MatchingStrategyChoice::Auto => MatchingStrategyChoice::Blossom,
            forced => forced,
        }
    }

    pub fn estimated_working_memory_bytes(&self, vertex_count: usize) -> Option<usize> {
        match self.selected_strategy(vertex_count) {
            MatchingStrategyChoice::BitDp => {
                self.bit_dp.estimated_working_memory_bytes(vertex_count)
            }
            MatchingStrategyChoice::Blossom => {
                self.blossom.estimated_working_memory_bytes(vertex_count)
            }
            MatchingStrategyChoice::Auto => unreachable!("auto is resolved above"),
        }
    }
}

impl PerfectMatchingStrategy for AutoPerfectMatching {
    fn strategy_name(&self, vertex_count: usize) -> &'static str {
        match self.selected_strategy(vertex_count) {
            MatchingStrategyChoice::BitDp => "bit_dp",
            MatchingStrategyChoice::Blossom => "blossom",
            MatchingStrategyChoice::Auto => unreachable!("auto is resolved above"),
        }
    }

    fn minimum_weight_perfect_matching(
        &self,
        vertices: &[usize],
        distances: &[Vec<u64>],
    ) -> Result<Vec<PerfectMatchingEdge>, SolverError> {
        match self.selected_strategy(vertices.len()) {
            MatchingStrategyChoice::BitDp => self
                .bit_dp
                .minimum_weight_perfect_matching(vertices, distances),
            MatchingStrategyChoice::Blossom => self
                .blossom
                .minimum_weight_perfect_matching(vertices, distances),
            MatchingStrategyChoice::Auto => unreachable!("auto is resolved above"),
        }
    }
}
