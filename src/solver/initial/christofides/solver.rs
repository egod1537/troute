use std::collections::BTreeSet;

use rand::RngCore;

use super::{
    eulerian_multigraph_tour, odd_degree_vertices, AutoPerfectMatching, PerfectMatchingEdge,
    PerfectMatchingStrategy,
};
use crate::solver::{
    minimum_spanning_tree, shortcut_euler_tour, AverageSymmetricDistance, InitialRouteGenerator,
    MstEdge, SolverError, SolverInput, SolverSolution, SymmetricDistanceStrategy,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChristofidesResult {
    pub symmetric_distance_strategy: String,
    pub matching_strategy: String,
    pub mst_edges: Vec<MstEdge>,
    pub odd_vertices: Vec<usize>,
    pub matching_edges: Vec<PerfectMatchingEdge>,
    pub eulerian_edges: Vec<MstEdge>,
    pub euler_tour: Vec<usize>,
    pub solution: SolverSolution,
}

#[derive(Debug, Clone)]
pub struct ChristofidesInitialRoute<S = AverageSymmetricDistance, P = AutoPerfectMatching> {
    symmetric_distance: S,
    perfect_matching: P,
}

impl Default for ChristofidesInitialRoute {
    fn default() -> Self {
        Self {
            symmetric_distance: AverageSymmetricDistance,
            perfect_matching: AutoPerfectMatching::default(),
        }
    }
}

impl<S, P> ChristofidesInitialRoute<S, P> {
    pub fn new(symmetric_distance: S, perfect_matching: P) -> Self {
        Self {
            symmetric_distance,
            perfect_matching,
        }
    }
}

impl<S: SymmetricDistanceStrategy, P: PerfectMatchingStrategy> ChristofidesInitialRoute<S, P> {
    pub fn generate_detailed(
        &self,
        input: SolverInput<'_>,
    ) -> Result<ChristofidesResult, SolverError> {
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let node_count = input.problem.locations().len();
        if input.matrix.size() != node_count {
            return Err(SolverError::MatrixSizeMismatch {
                matrix: input.matrix.size(),
                locations: node_count,
            });
        }
        let mst_edges = minimum_spanning_tree(node_count, |left, right| {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            self.symmetric_distance
                .symmetric_distance(input.matrix, left, right)
        })?;
        let odd_vertices = odd_degree_vertices(node_count, &mst_edges)?;
        debug_assert_eq!(odd_vertices.len() % 2, 0);
        let mut odd_distances = vec![vec![0_u64; odd_vertices.len()]; odd_vertices.len()];
        for left in 0..odd_vertices.len() {
            for right in left + 1..odd_vertices.len() {
                let distance = self.symmetric_distance.symmetric_distance(
                    input.matrix,
                    odd_vertices[left],
                    odd_vertices[right],
                )?;
                odd_distances[left][right] = distance;
                odd_distances[right][left] = distance;
            }
        }
        let matching_edges = self
            .perfect_matching
            .minimum_weight_perfect_matching(&odd_vertices, &odd_distances)?;
        validate_matching_result(&odd_vertices, &matching_edges)?;

        let mut eulerian_edges = mst_edges.clone();
        eulerian_edges.extend(matching_edges.iter().map(|edge| MstEdge {
            from: edge.left,
            to: edge.right,
            distance: edge.distance,
        }));
        let euler_tour = eulerian_multigraph_tour(node_count, &eulerian_edges, 0)?;
        let mut visit_order = shortcut_euler_tour(node_count, &euler_tour)?;
        move_fixed_destination_to_end(&mut visit_order, input.problem.end_location_index())?;

        Ok(ChristofidesResult {
            symmetric_distance_strategy: self.symmetric_distance.name().to_owned(),
            matching_strategy: self
                .perfect_matching
                .strategy_name(odd_vertices.len())
                .to_owned(),
            mst_edges,
            odd_vertices,
            matching_edges,
            eulerian_edges,
            euler_tour,
            solution: SolverSolution { visit_order },
        })
    }
}

impl<S: SymmetricDistanceStrategy, P: PerfectMatchingStrategy> InitialRouteGenerator
    for ChristofidesInitialRoute<S, P>
{
    fn generate(
        &self,
        input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        self.generate_detailed(input).map(|result| result.solution)
    }
}

fn validate_matching_result(
    odd_vertices: &[usize],
    matching: &[PerfectMatchingEdge],
) -> Result<(), SolverError> {
    if matching.len() * 2 != odd_vertices.len() {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching does not cover every odd vertex".to_owned(),
        ));
    }
    let expected: BTreeSet<_> = odd_vertices.iter().copied().collect();
    let mut actual = BTreeSet::new();
    for edge in matching {
        if edge.left == edge.right || !actual.insert(edge.left) || !actual.insert(edge.right) {
            return Err(SolverError::InvalidConfiguration(
                "perfect matching must use every odd vertex exactly once".to_owned(),
            ));
        }
    }
    if actual != expected {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching contains a non-odd vertex".to_owned(),
        ));
    }
    Ok(())
}

fn move_fixed_destination_to_end(
    visit_order: &mut Vec<usize>,
    destination: usize,
) -> Result<(), SolverError> {
    if visit_order.len() <= 1 {
        return Ok(());
    }
    let position = visit_order
        .iter()
        .position(|&vertex| vertex == destination)
        .ok_or(SolverError::InvalidVisitOrder)?;
    visit_order.remove(position);
    visit_order.push(destination);
    Ok(())
}
