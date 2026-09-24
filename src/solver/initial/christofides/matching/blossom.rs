use std::panic::{catch_unwind, AssertUnwindSafe};

use integer_blossom::min_weight_perfect_matching;

use crate::solver::SolverError;

use super::{validate_matching_problem, PerfectMatchingEdge, PerfectMatchingStrategy};

/// Adapter around the maintained `integer-blossom` crate. The upstream solver
/// is deterministic and implements O(V^3) primal-dual weighted Blossom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlossomPerfectMatching;

impl BlossomPerfectMatching {
    /// Size of the flattened i128 input buffer owned by this adapter. The
    /// upstream crate additionally reuses O(n^2) thread-local solver buffers.
    pub fn estimated_working_memory_bytes(&self, vertex_count: usize) -> Option<usize> {
        vertex_count
            .checked_mul(vertex_count)?
            .checked_mul(std::mem::size_of::<i128>())
    }
}

impl PerfectMatchingStrategy for BlossomPerfectMatching {
    fn strategy_name(&self, _vertex_count: usize) -> &'static str {
        "blossom"
    }

    fn minimum_weight_perfect_matching(
        &self,
        vertices: &[usize],
        distances: &[Vec<u64>],
    ) -> Result<Vec<PerfectMatchingEdge>, SolverError> {
        validate_matching_problem(vertices, distances)?;
        if vertices.is_empty() {
            return Ok(Vec::new());
        }
        let vertex_count = vertices.len();
        let costs: Vec<i128> = distances
            .iter()
            .flat_map(|row| row.iter().map(|&distance| i128::from(distance)))
            .collect();
        let mates = catch_unwind(AssertUnwindSafe(|| {
            min_weight_perfect_matching(&costs, vertex_count)
        }))
        .map_err(|_| {
            SolverError::PerfectMatchingFailed(
                "integer-blossom panicked while solving the matching".to_owned(),
            )
        })?;
        validate_blossom_mates(&mates, vertex_count)?;

        Ok((0..vertex_count)
            .filter_map(|left| {
                let right = mates[left];
                (left < right).then_some(PerfectMatchingEdge {
                    left: vertices[left],
                    right: vertices[right],
                    distance: distances[left][right],
                })
            })
            .collect())
    }
}

fn validate_blossom_mates(mates: &[usize], vertex_count: usize) -> Result<(), SolverError> {
    if mates.len() != vertex_count {
        return Err(SolverError::PerfectMatchingFailed(format!(
            "integer-blossom returned {} mates for {vertex_count} vertices",
            mates.len()
        )));
    }
    for (vertex, &mate) in mates.iter().enumerate() {
        if mate >= vertex_count || mate == vertex || mates.get(mate) != Some(&vertex) {
            return Err(SolverError::PerfectMatchingFailed(format!(
                "integer-blossom returned an invalid mate for vertex {vertex}"
            )));
        }
    }
    Ok(())
}
