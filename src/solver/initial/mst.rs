mod distance;
mod euler;
mod tree;

use rand::RngCore;

use super::{validate_generator_input, InitialRouteGenerator};
use crate::solver::{SolverError, SolverInput, SolverSolution};

pub use distance::*;
pub use euler::*;
pub(crate) use tree::minimum_spanning_tree;
pub use tree::MstEdge;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MstDoubleTreeResult {
    pub symmetric_distance_strategy: String,
    pub mst_edges: Vec<MstEdge>,
    pub euler_tour: Vec<usize>,
    pub solution: SolverSolution,
}

/// Deterministic MST double-tree route generator. The symmetric strategy is
/// used only for the MST; route evaluation always remains a caller concern and
/// uses the original directed matrix.
#[derive(Debug, Clone)]
pub struct MstDoubleTreeInitialRoute<S = AverageSymmetricDistance> {
    symmetric_distance: S,
}

impl Default for MstDoubleTreeInitialRoute {
    fn default() -> Self {
        Self {
            symmetric_distance: AverageSymmetricDistance,
        }
    }
}

impl<S> MstDoubleTreeInitialRoute<S> {
    pub fn new(symmetric_distance: S) -> Self {
        Self { symmetric_distance }
    }
}

impl<S: SymmetricDistanceStrategy> MstDoubleTreeInitialRoute<S> {
    pub fn generate_detailed(
        &self,
        input: SolverInput<'_>,
    ) -> Result<MstDoubleTreeResult, SolverError> {
        validate_generator_input(&input)?;
        let location_count = input.problem.locations().len();
        let edges = minimum_spanning_tree(location_count, |left, right| {
            self.symmetric_distance
                .symmetric_distance(input.matrix, left, right)
        })?;
        let euler_tour = doubled_tree_euler_tour(location_count, &edges, 0)?;
        let mut visit_order = shortcut_euler_tour(location_count, &euler_tour)?;

        // troute models a Hamiltonian path with distinct fixed endpoints, not
        // a TSP cycle. Preserve the endpoint contract after shortcutting.
        if location_count > 1 {
            let end = input.problem.end_location_index();
            let position = visit_order
                .iter()
                .position(|&location| location == end)
                .ok_or(SolverError::InvalidVisitOrder)?;
            visit_order.remove(position);
            visit_order.push(end);
        }

        input.problem.restore_order_locks(&mut visit_order);
        Ok(MstDoubleTreeResult {
            symmetric_distance_strategy: self.symmetric_distance.name().to_owned(),
            mst_edges: edges,
            euler_tour,
            solution: SolverSolution { visit_order },
        })
    }
}

impl<S: SymmetricDistanceStrategy> InitialRouteGenerator for MstDoubleTreeInitialRoute<S> {
    fn generate(
        &self,
        input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        self.generate_detailed(input).map(|result| result.solution)
    }
}

#[cfg(test)]
mod tests;
