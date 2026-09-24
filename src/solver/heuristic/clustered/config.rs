use crate::solver::{SolverError, EXACT_MAX_LOCATIONS};

pub const DEFAULT_MAX_EXACT_CLUSTER_SIZE: usize = 10;
pub const MAX_EXACT_CLUSTER_SIZE: usize = EXACT_MAX_LOCATIONS;
/// Two internal positions are reserved for the previous anchor and the
/// next-cluster exit proxy, so configured larger clusters are safely split at
/// this effective member count.
pub(super) const MAX_CLUSTER_MEMBERS_WITH_ANCHORS: usize = EXACT_MAX_LOCATIONS - 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusteredSolverConfig {
    pub max_cluster_size: usize,
}

impl ClusteredSolverConfig {
    pub fn new(max_cluster_size: usize) -> Result<Self, SolverError> {
        if !(1..=MAX_EXACT_CLUSTER_SIZE).contains(&max_cluster_size) {
            return Err(SolverError::InvalidConfiguration(format!(
                "max_cluster_size must be between 1 and {MAX_EXACT_CLUSTER_SIZE}"
            )));
        }
        Ok(Self { max_cluster_size })
    }
}

impl Default for ClusteredSolverConfig {
    fn default() -> Self {
        Self {
            max_cluster_size: DEFAULT_MAX_EXACT_CLUSTER_SIZE,
        }
    }
}
