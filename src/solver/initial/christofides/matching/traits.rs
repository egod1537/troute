use std::collections::BTreeSet;

use crate::solver::SolverError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerfectMatchingEdge {
    pub left: usize,
    pub right: usize,
    pub distance: u64,
}

/// Computes a minimum-weight perfect matching over `vertices`. `distances` is
/// indexed by positions in `vertices`, not by global location indices.
pub trait PerfectMatchingStrategy: Send + Sync {
    fn strategy_name(&self, _vertex_count: usize) -> &'static str {
        "custom"
    }

    fn minimum_weight_perfect_matching(
        &self,
        vertices: &[usize],
        distances: &[Vec<u64>],
    ) -> Result<Vec<PerfectMatchingEdge>, SolverError>;
}

pub(super) fn validate_matching_problem(
    vertices: &[usize],
    distances: &[Vec<u64>],
) -> Result<(), SolverError> {
    if !vertices.len().is_multiple_of(2) {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching requires an even number of vertices".to_owned(),
        ));
    }
    if vertices.iter().copied().collect::<BTreeSet<_>>().len() != vertices.len() {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching vertices must be unique".to_owned(),
        ));
    }
    if distances.len() != vertices.len() || distances.iter().any(|row| row.len() != vertices.len())
    {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching distance matrix must match the vertex count".to_owned(),
        ));
    }
    for (left, row) in distances.iter().enumerate() {
        for (right, reverse_row) in distances.iter().enumerate().skip(left + 1) {
            if row[right] != reverse_row[left] {
                return Err(SolverError::InvalidConfiguration(
                    "perfect matching distances must be symmetric".to_owned(),
                ));
            }
        }
    }
    Ok(())
}
