use std::collections::BTreeSet;

use crate::{
    domain::{OptimizationProblem, TimeOfDay},
    matrix::TravelTimeMatrix,
    solver::{
        doubled_tree_euler_tour, minimum_spanning_tree, shortcut_euler_tour,
        AverageSymmetricDistance, ExactBitDpSolver, RouteSolver, SolverError, SolverInput,
        SolverSolution, SymmetricDistanceStrategy, EXACT_MAX_LOCATIONS,
    },
};

use super::ClusteredSolver;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    pub(super) members: Vec<usize>,
}

impl Cluster {
    pub fn new(members: Vec<usize>) -> Self {
        Self { members }
    }

    pub fn members(&self) -> &[usize] {
        &self.members
    }
}

pub trait ClusterStrategy: Send + Sync {
    fn name(&self) -> &'static str {
        "custom"
    }

    fn cluster(
        &self,
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
        max_cluster_size: usize,
    ) -> Result<Vec<Cluster>, SolverError>;
}

/// Deterministic directed-nearest-neighbor clustering using only the supplied
/// matrix. No routing lookup occurs inside this strategy.
#[derive(Debug, Clone, Copy, Default)]
pub struct TravelTimeClusterStrategy;

impl ClusterStrategy for TravelTimeClusterStrategy {
    fn name(&self) -> &'static str {
        "directed_nearest_neighbor"
    }

    fn cluster(
        &self,
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
        max_cluster_size: usize,
    ) -> Result<Vec<Cluster>, SolverError> {
        if max_cluster_size == 0 {
            return Err(SolverError::InvalidConfiguration(
                "max_cluster_size must be positive".to_owned(),
            ));
        }
        let end = problem.end_location_index();
        let mut remaining: BTreeSet<_> = (1..end).collect();
        let mut clusters = Vec::new();
        while let Some(&seed) = remaining.first() {
            remaining.remove(&seed);
            let mut members = vec![seed];
            let mut current = seed;
            while members.len() < max_cluster_size && !remaining.is_empty() {
                let next = remaining.iter().copied().min_by_key(|&candidate| {
                    (
                        matrix
                            .travel_minutes(current, candidate)
                            .unwrap_or(u32::MAX),
                        candidate,
                    )
                });
                let Some(next) = next else { break };
                remaining.remove(&next);
                members.push(next);
                current = next;
            }
            clusters.push(Cluster::new(members));
        }
        Ok(clusters)
    }
}

pub trait ClusterOrderStrategy: Send + Sync {
    fn name(&self) -> &'static str {
        "custom"
    }

    /// Returns indices into `clusters` in visit order.
    fn order(
        &self,
        clusters: &[Cluster],
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
    ) -> Result<Vec<usize>, SolverError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GreedyClusterOrderStrategy;

impl ClusterOrderStrategy for GreedyClusterOrderStrategy {
    fn name(&self) -> &'static str {
        "greedy_bridge"
    }

    fn order(
        &self,
        clusters: &[Cluster],
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
    ) -> Result<Vec<usize>, SolverError> {
        let mut remaining: BTreeSet<_> = (0..clusters.len()).collect();
        let mut ordered = Vec::with_capacity(clusters.len());
        let mut previous = vec![problem.start_location_index()];
        while !remaining.is_empty() {
            let next = remaining
                .iter()
                .copied()
                .min_by_key(|&cluster_index| {
                    let bridge = previous
                        .iter()
                        .flat_map(|&from| {
                            clusters[cluster_index]
                                .members
                                .iter()
                                .map(move |&to| matrix.travel_minutes(from, to).unwrap_or(u32::MAX))
                        })
                        .min()
                        .unwrap_or(u32::MAX);
                    (bridge, cluster_index)
                })
                .ok_or_else(|| SolverError::Failed("cluster ordering failed".to_owned()))?;
            remaining.remove(&next);
            previous.clone_from(&clusters[next].members);
            ordered.push(next);
        }
        Ok(ordered)
    }
}

/// Optional cluster ordering that treats the start and each cluster as nodes,
/// then shortcuts a doubled MST walk. Inter-cluster weights are the minimum
/// symmetric distance between their member locations.
#[derive(Debug, Clone)]
pub struct MstClusterOrderStrategy<S = AverageSymmetricDistance> {
    symmetric_distance: S,
}

impl Default for MstClusterOrderStrategy {
    fn default() -> Self {
        Self {
            symmetric_distance: AverageSymmetricDistance,
        }
    }
}

impl<S> MstClusterOrderStrategy<S> {
    pub fn new(symmetric_distance: S) -> Self {
        Self { symmetric_distance }
    }
}

impl<S: SymmetricDistanceStrategy> ClusterOrderStrategy for MstClusterOrderStrategy<S> {
    fn name(&self) -> &'static str {
        "mst_double_tree"
    }

    fn order(
        &self,
        clusters: &[Cluster],
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
    ) -> Result<Vec<usize>, SolverError> {
        if clusters.is_empty() {
            return Ok(Vec::new());
        }
        if matrix.size() != problem.locations().len() {
            return Err(SolverError::MatrixSizeMismatch {
                matrix: matrix.size(),
                locations: problem.locations().len(),
            });
        }
        let node_count = clusters.len() + 1;
        let edges = minimum_spanning_tree(node_count, |left, right| {
            let left_members: &[usize] = if left == 0 {
                std::slice::from_ref(&0)
            } else {
                clusters[left - 1].members()
            };
            let right_members: &[usize] = if right == 0 {
                std::slice::from_ref(&0)
            } else {
                clusters[right - 1].members()
            };
            let mut best = None;
            for &from in left_members {
                for &to in right_members {
                    let distance = self
                        .symmetric_distance
                        .symmetric_distance(matrix, from, to)?;
                    best = Some(best.map_or(distance, |current: u64| current.min(distance)));
                }
            }
            best.ok_or_else(|| SolverError::Failed("empty cluster cannot be ordered".to_owned()))
        })?;
        let euler = doubled_tree_euler_tour(node_count, &edges, 0)?;
        Ok(shortcut_euler_tour(node_count, &euler)?
            .into_iter()
            .filter(|&node| node != 0)
            .map(|node| node - 1)
            .collect())
    }
}
pub(super) fn validate_clusters(
    clusters: &[Cluster],
    problem: &OptimizationProblem,
    max_cluster_size: usize,
) -> Result<(), SolverError> {
    let end = problem.end_location_index();
    let mut seen = vec![false; problem.locations().len()];
    for cluster in clusters {
        if cluster.members.is_empty() || cluster.members.len() > max_cluster_size {
            return Err(SolverError::InvalidConfiguration(
                "cluster strategy returned an invalid cluster size".to_owned(),
            ));
        }
        for &member in &cluster.members {
            if member == 0 || member >= end || seen[member] {
                return Err(SolverError::InvalidConfiguration(
                    "cluster strategy must include each intermediate location exactly once"
                        .to_owned(),
                ));
            }
            seen[member] = true;
        }
    }
    if (1..end).any(|member| !seen[member]) {
        return Err(SolverError::InvalidConfiguration(
            "cluster strategy omitted an intermediate location".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_cluster_order(
    order: &[usize],
    cluster_count: usize,
) -> Result<(), SolverError> {
    if order.len() != cluster_count {
        return Err(SolverError::InvalidConfiguration(
            "cluster order has the wrong length".to_owned(),
        ));
    }
    let mut seen = vec![false; cluster_count];
    for &index in order {
        if index >= cluster_count || seen[index] {
            return Err(SolverError::InvalidConfiguration(
                "cluster order must include each cluster exactly once".to_owned(),
            ));
        }
        seen[index] = true;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolverKind {
    Exact,
    Clustered,
}

pub trait SolverSelectionPolicy: Send + Sync {
    fn select(&self, location_count: usize) -> SolverKind;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThresholdSolverSelectionPolicy {
    exact_limit: usize,
}

impl ThresholdSolverSelectionPolicy {
    pub fn new(exact_limit: usize) -> Result<Self, SolverError> {
        if exact_limit == 0 || exact_limit > EXACT_MAX_LOCATIONS {
            return Err(SolverError::InvalidConfiguration(format!(
                "exact_limit must be between 1 and {EXACT_MAX_LOCATIONS}"
            )));
        }
        Ok(Self { exact_limit })
    }

    pub fn exact_limit(self) -> usize {
        self.exact_limit
    }
}

impl Default for ThresholdSolverSelectionPolicy {
    fn default() -> Self {
        Self {
            exact_limit: EXACT_MAX_LOCATIONS,
        }
    }
}

impl SolverSelectionPolicy for ThresholdSolverSelectionPolicy {
    fn select(&self, location_count: usize) -> SolverKind {
        if location_count <= self.exact_limit {
            SolverKind::Exact
        } else {
            SolverKind::Clustered
        }
    }
}

#[derive(Debug, Clone)]
pub struct AutomaticRouteSolver<
    E = ExactBitDpSolver,
    H = ClusteredSolver,
    P = ThresholdSolverSelectionPolicy,
> {
    exact: E,
    clustered: H,
    selection: P,
}

impl Default for AutomaticRouteSolver {
    fn default() -> Self {
        Self {
            exact: ExactBitDpSolver::default(),
            clustered: ClusteredSolver::default(),
            selection: ThresholdSolverSelectionPolicy::default(),
        }
    }
}

impl<E, H, P> AutomaticRouteSolver<E, H, P> {
    pub fn new(exact: E, clustered: H, selection: P) -> Self {
        Self {
            exact,
            clustered,
            selection,
        }
    }
}

impl<E: RouteSolver, H: RouteSolver, P: SolverSelectionPolicy> RouteSolver
    for AutomaticRouteSolver<E, H, P>
{
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        match self.selection.select(input.problem.locations().len()) {
            SolverKind::Exact => self.exact.solve(input),
            SolverKind::Clustered => self.clustered.solve(input),
        }
    }

    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        match self.selection.select(input.problem.locations().len()) {
            SolverKind::Exact => self.exact.selected_start_time(input, solution),
            SolverKind::Clustered => self.clustered.selected_start_time(input, solution),
        }
    }
}
