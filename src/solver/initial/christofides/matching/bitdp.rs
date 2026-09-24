use crate::solver::SolverError;

use super::{validate_matching_problem, PerfectMatchingEdge, PerfectMatchingStrategy};

pub const MAX_BIT_DP_MATCHING_VERTICES: usize = 20;

/// Exact minimum-weight perfect matching using `dp[matched_mask]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitDpPerfectMatching {
    max_vertices: usize,
}

impl Default for BitDpPerfectMatching {
    fn default() -> Self {
        Self {
            max_vertices: MAX_BIT_DP_MATCHING_VERTICES,
        }
    }
}

impl BitDpPerfectMatching {
    pub fn new(max_vertices: usize) -> Result<Self, SolverError> {
        if max_vertices > MAX_BIT_DP_MATCHING_VERTICES {
            return Err(SolverError::InvalidConfiguration(format!(
                "bit-DP perfect matching limit must not exceed {MAX_BIT_DP_MATCHING_VERTICES}"
            )));
        }
        Ok(Self { max_vertices })
    }

    pub fn max_vertices(&self) -> usize {
        self.max_vertices
    }

    pub fn estimated_working_memory_bytes(&self, vertex_count: usize) -> Option<usize> {
        let state_count = 1_usize.checked_shl(vertex_count as u32)?;
        state_count.checked_mul(
            std::mem::size_of::<u64>() + std::mem::size_of::<Option<(usize, usize, usize)>>(),
        )
    }
}

impl PerfectMatchingStrategy for BitDpPerfectMatching {
    fn strategy_name(&self, _vertex_count: usize) -> &'static str {
        "bit_dp"
    }

    fn minimum_weight_perfect_matching(
        &self,
        vertices: &[usize],
        distances: &[Vec<u64>],
    ) -> Result<Vec<PerfectMatchingEdge>, SolverError> {
        validate_matching_problem(vertices, distances)?;
        let vertex_count = vertices.len();
        if vertex_count > self.max_vertices {
            return Err(SolverError::UnsupportedMatchingVertexCount {
                maximum: self.max_vertices,
                actual: vertex_count,
            });
        }
        if vertex_count == 0 {
            return Ok(Vec::new());
        }

        let state_count = 1_usize << vertex_count;
        let full_mask = state_count - 1;
        let mut dp = vec![u64::MAX; state_count];
        let mut predecessor = vec![None; state_count];
        dp[0] = 0;

        for mask in 0..state_count {
            if dp[mask] == u64::MAX || mask == full_mask {
                continue;
            }
            let unmatched = full_mask ^ mask;
            let first = unmatched.trailing_zeros() as usize;
            let first_bit = 1_usize << first;
            for (second, &pair_distance) in distances[first].iter().enumerate().skip(first + 1) {
                let second_bit = 1_usize << second;
                if unmatched & second_bit == 0 {
                    continue;
                }
                let next_mask = mask | first_bit | second_bit;
                let candidate = dp[mask]
                    .checked_add(pair_distance)
                    .ok_or_else(|| SolverError::Failed("matching cost overflow".to_owned()))?;
                if candidate < dp[next_mask] {
                    dp[next_mask] = candidate;
                    predecessor[next_mask] = Some((mask, first, second));
                }
            }
        }

        if dp[full_mask] == u64::MAX {
            return Err(SolverError::Failed(
                "perfect matching could not be constructed".to_owned(),
            ));
        }
        let mut matching = Vec::with_capacity(vertex_count / 2);
        let mut mask = full_mask;
        while mask != 0 {
            let (previous, left, right) = predecessor[mask].ok_or_else(|| {
                SolverError::Failed("perfect matching predecessor is missing".to_owned())
            })?;
            matching.push(PerfectMatchingEdge {
                left: vertices[left],
                right: vertices[right],
                distance: distances[left][right],
            });
            mask = previous;
        }
        matching.reverse();
        Ok(matching)
    }
}
