use thiserror::Error;

#[derive(Debug, Error)]
pub enum SolverError {
    #[error("solver was cancelled")]
    Cancelled,
    #[error("no feasible route was found")]
    NoFeasibleRoute,
    #[error("exact solver supports at most {maximum} locations (actual {actual})")]
    UnsupportedLocationCount { maximum: usize, actual: usize },
    #[error("bit-DP perfect matching supports at most {maximum} vertices (actual {actual})")]
    UnsupportedMatchingVertexCount { maximum: usize, actual: usize },
    #[error("minimum-weight perfect matching failed: {0}")]
    PerfectMatchingFailed(String),
    #[error("travel time matrix size {matrix} does not match {locations} locations")]
    MatrixSizeMismatch { matrix: usize, locations: usize },
    #[error("solver produced an invalid visit order")]
    InvalidVisitOrder,
    #[error("invalid solver configuration: {0}")]
    InvalidConfiguration(String),
    #[error("solver failed: {0}")]
    Failed(String),
}
