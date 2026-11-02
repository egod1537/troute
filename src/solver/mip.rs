use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipMipConfig {
    pub time_limit: Duration,
    pub threads: usize,
    pub memory_mb: usize,
    pub seed: u64,
}

#[cfg(feature = "solver-mip-scip")]
mod scip;

#[cfg(feature = "solver-mip-scip")]
pub use scip::ScipMipStrategy;
