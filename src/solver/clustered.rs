use std::{cmp::Ordering, collections::BTreeSet};

use crate::{
    domain::{Location, OptimizationProblem, TimeOfDay, TimeWindow},
    matrix::TravelTimeMatrix,
};

use super::{
    doubled_tree_euler_tour, evaluate_solution, minimum_spanning_tree, shortcut_euler_tour,
    transition_time, AverageSymmetricDistance, DefaultObjectivePolicy, ExactBitDpSolver,
    FrontierPolicy, ObjectivePolicy, RouteSolver, SolutionMetrics, SolverError, SolverInput,
    SolverSolution, SymmetricDistanceStrategy, TimeCostFrontierPolicy, EXACT_MAX_LOCATIONS,
    TIME_SLOT_MINUTES,
};

pub const DEFAULT_MAX_EXACT_CLUSTER_SIZE: usize = 10;
pub const MAX_EXACT_CLUSTER_SIZE: usize = EXACT_MAX_LOCATIONS;
/// Two internal positions are reserved for the previous anchor and the
/// next-cluster exit proxy, so configured larger clusters are safely split at
/// this effective member count.
const MAX_CLUSTER_MEMBERS_WITH_ANCHORS: usize = EXACT_MAX_LOCATIONS - 2;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    members: Vec<usize>,
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

pub trait LocalImprovementStrategy: Send + Sync {
    fn name(&self) -> &'static str {
        "custom"
    }

    fn enabled_operations(&self) -> LocalImprovementOperations {
        LocalImprovementOperations::default()
    }

    fn improve(
        &self,
        route: &mut Vec<usize>,
        cluster_boundaries: &[usize],
        input: SolverInput<'_>,
        objective: &dyn ObjectivePolicy,
    ) -> Result<LocalImprovementResult, SolverError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalImprovementResult {
    pub metrics: SolutionMetrics,
    pub accepted_moves: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalImprovementOperations {
    pub swap: bool,
    pub relocate: bool,
    pub two_opt: bool,
}

/// Tries swaps between the last member of one cluster and the first member of
/// the next cluster. A move is accepted only when the shared objective says it
/// is strictly better and the complete route remains feasible.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoundarySwapLocalImprovement;

impl LocalImprovementStrategy for BoundarySwapLocalImprovement {
    fn name(&self) -> &'static str {
        "boundary_swap"
    }

    fn enabled_operations(&self) -> LocalImprovementOperations {
        LocalImprovementOperations {
            swap: true,
            relocate: false,
            two_opt: false,
        }
    }

    fn improve(
        &self,
        route: &mut Vec<usize>,
        cluster_boundaries: &[usize],
        input: SolverInput<'_>,
        objective: &dyn ObjectivePolicy,
    ) -> Result<LocalImprovementResult, SolverError> {
        let mut metrics = evaluate_solution(
            objective,
            SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: input.cancellation,
            },
            &SolverSolution {
                visit_order: route.clone(),
            },
        )?;
        let mut accepted_moves = 0;
        for &boundary in cluster_boundaries {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            if boundary == 0 || boundary + 1 >= route.len() - 1 {
                continue;
            }
            route.swap(boundary, boundary + 1);
            let candidate_solution = SolverSolution {
                visit_order: route.clone(),
            };
            let candidate = evaluate_solution(
                objective,
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                &candidate_solution,
            );
            match candidate {
                Ok(candidate) if objective.compare(&candidate, &metrics) == Ordering::Less => {
                    metrics = candidate;
                    accepted_moves += 1;
                }
                Ok(_) | Err(SolverError::NoFeasibleRoute) => route.swap(boundary, boundary + 1),
                Err(error) => return Err(error),
            }
        }
        Ok(LocalImprovementResult {
            metrics,
            accepted_moves,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusteredSolverStats {
    pub cluster_count: usize,
    pub cluster_sizes: Vec<usize>,
    pub cluster_strategy: String,
    pub cluster_order_strategy: String,
    pub cluster_order: Vec<usize>,
    pub cluster_details: Vec<ClusterSolveStats>,
    pub exact_generated_states: usize,
    pub exact_frontier_states: usize,
    pub accepted_local_moves: usize,
    pub improvement_strategy: String,
    pub improvement_operations: LocalImprovementOperations,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterSolveStats {
    /// Zero-based index into the clusters returned by the cluster strategy.
    pub cluster_index: usize,
    pub members: Vec<usize>,
    pub route: Vec<usize>,
    pub entry: Option<usize>,
    pub exit: Option<usize>,
    pub exact_generated_states: usize,
    pub exact_frontier_states: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusteredSolveResult {
    pub solution: SolverSolution,
    pub metrics_before_improvement: SolutionMetrics,
    pub metrics: SolutionMetrics,
    pub stats: ClusteredSolverStats,
}

#[derive(Debug, Clone)]
pub struct ClusteredSolver<
    C = TravelTimeClusterStrategy,
    R = GreedyClusterOrderStrategy,
    L = BoundarySwapLocalImprovement,
    O = DefaultObjectivePolicy,
    F = TimeCostFrontierPolicy,
> {
    config: ClusteredSolverConfig,
    cluster_strategy: C,
    order_strategy: R,
    local_improvement: L,
    exact: ExactBitDpSolver<O, F>,
}

impl Default for ClusteredSolver {
    fn default() -> Self {
        Self {
            config: ClusteredSolverConfig::default(),
            cluster_strategy: TravelTimeClusterStrategy,
            order_strategy: GreedyClusterOrderStrategy,
            local_improvement: BoundarySwapLocalImprovement,
            exact: ExactBitDpSolver::default(),
        }
    }
}

impl ClusteredSolver {
    pub fn with_config(config: ClusteredSolverConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }
}

impl<C, R, L, O, F> ClusteredSolver<C, R, L, O, F> {
    pub fn new(
        config: ClusteredSolverConfig,
        cluster_strategy: C,
        order_strategy: R,
        local_improvement: L,
        exact: ExactBitDpSolver<O, F>,
    ) -> Self {
        Self {
            config,
            cluster_strategy,
            order_strategy,
            local_improvement,
            exact,
        }
    }
}

impl<
        C: ClusterStrategy,
        R: ClusterOrderStrategy,
        L: LocalImprovementStrategy,
        O: ObjectivePolicy,
        F: FrontierPolicy,
    > ClusteredSolver<C, R, L, O, F>
{
    pub fn solve_detailed(
        &self,
        input: SolverInput<'_>,
    ) -> Result<ClusteredSolveResult, SolverError> {
        self.validate_input(&input)?;
        if input.problem.locations().len() == 1 {
            let solution = SolverSolution {
                visit_order: vec![0],
            };
            let metrics = evaluate_solution(
                &self.exact.objective,
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                &solution,
            )?;
            return Ok(ClusteredSolveResult {
                solution,
                metrics_before_improvement: metrics,
                metrics,
                stats: ClusteredSolverStats {
                    cluster_count: 0,
                    cluster_sizes: Vec::new(),
                    cluster_strategy: self.cluster_strategy.name().to_owned(),
                    cluster_order_strategy: self.order_strategy.name().to_owned(),
                    cluster_order: Vec::new(),
                    cluster_details: Vec::new(),
                    exact_generated_states: 0,
                    exact_frontier_states: 0,
                    accepted_local_moves: 0,
                    improvement_strategy: self.local_improvement.name().to_owned(),
                    improvement_operations: self.local_improvement.enabled_operations(),
                },
            });
        }
        let effective_cluster_size = self
            .config
            .max_cluster_size
            .min(MAX_CLUSTER_MEMBERS_WITH_ANCHORS);
        let clusters =
            self.cluster_strategy
                .cluster(input.problem, input.matrix, effective_cluster_size)?;
        validate_clusters(&clusters, input.problem, effective_cluster_size)?;
        let cluster_order = self
            .order_strategy
            .order(&clusters, input.problem, input.matrix)?;
        validate_cluster_order(&cluster_order, clusters.len())?;

        let mut route = vec![input.problem.start_location_index()];
        let mut boundaries = Vec::new();
        let mut current_location = input.problem.start_location_index();
        let mut current_slot =
            super::minutes_to_slot_ceil(u32::from(input.problem.start_time().minutes())) as u16;
        let mut exact_generated_states = 0;
        let mut exact_frontier_states = 0;
        let mut cluster_details = Vec::with_capacity(clusters.len());

        for (position, &cluster_index) in cluster_order.iter().enumerate() {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            let cluster = &clusters[cluster_index];
            let next_candidates: Vec<_> = cluster_order
                .get(position + 1)
                .map(|&next| clusters[next].members.clone())
                .unwrap_or_else(|| vec![input.problem.end_location_index()]);
            let solved = self.solve_cluster(
                &input,
                current_location,
                current_slot,
                cluster,
                &next_candidates,
            )?;
            exact_generated_states += solved.generated_states;
            exact_frontier_states += solved.frontier_states;
            cluster_details.push(ClusterSolveStats {
                cluster_index,
                members: cluster.members.clone(),
                entry: solved.order.first().copied(),
                exit: solved.order.last().copied(),
                route: solved.order.clone(),
                exact_generated_states: solved.generated_states,
                exact_frontier_states: solved.frontier_states,
            });
            for member in solved.order {
                current_slot = transition_time(&input, current_location, member, current_slot)
                    .ok_or(SolverError::NoFeasibleRoute)?;
                current_location = member;
                route.push(member);
            }
            if position + 1 < cluster_order.len() {
                boundaries.push(route.len() - 1);
            }
        }
        route.push(input.problem.end_location_index());

        let initial_solution = SolverSolution {
            visit_order: route.clone(),
        };
        let metrics_before_improvement = evaluate_solution(
            &self.exact.objective,
            SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: input.cancellation,
            },
            &initial_solution,
        )?;
        let improvement = self.local_improvement.improve(
            &mut route,
            &boundaries,
            SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: input.cancellation,
            },
            &self.exact.objective,
        )?;
        let solution = SolverSolution { visit_order: route };
        // Final full-route validation is intentionally mandatory even when the
        // local strategy reports its own metrics.
        let metrics = evaluate_solution(
            &self.exact.objective,
            SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: input.cancellation,
            },
            &solution,
        )?;
        debug_assert_eq!(metrics, improvement.metrics);

        Ok(ClusteredSolveResult {
            solution,
            metrics_before_improvement,
            metrics,
            stats: ClusteredSolverStats {
                cluster_count: clusters.len(),
                cluster_sizes: clusters
                    .iter()
                    .map(|cluster| cluster.members.len())
                    .collect(),
                cluster_strategy: self.cluster_strategy.name().to_owned(),
                cluster_order_strategy: self.order_strategy.name().to_owned(),
                cluster_order,
                cluster_details,
                exact_generated_states,
                exact_frontier_states,
                accepted_local_moves: improvement.accepted_moves,
                improvement_strategy: self.local_improvement.name().to_owned(),
                improvement_operations: self.local_improvement.enabled_operations(),
            },
        })
    }

    fn validate_input(&self, input: &SolverInput<'_>) -> Result<(), SolverError> {
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let location_count = input.problem.locations().len();
        if input.matrix.size() != location_count {
            return Err(SolverError::MatrixSizeMismatch {
                matrix: input.matrix.size(),
                locations: location_count,
            });
        }
        ClusteredSolverConfig::new(self.config.max_cluster_size)?;
        Ok(())
    }

    fn solve_cluster(
        &self,
        input: &SolverInput<'_>,
        previous: usize,
        start_slot: u16,
        cluster: &Cluster,
        next_candidates: &[usize],
    ) -> Result<SolvedCluster, SolverError> {
        let dummy = Location::new(
            "__troute_cluster_exit__".to_owned(),
            input.problem.locations()[previous]
                .routing_reference()
                .clone(),
            TimeWindow::new(
                TimeOfDay::from_minutes(0)
                    .map_err(|error| SolverError::Failed(error.to_string()))?,
                TimeOfDay::from_minutes(1439)
                    .map_err(|error| SolverError::Failed(error.to_string()))?,
            )
            .map_err(|error| SolverError::Failed(error.to_string()))?,
            0,
        );
        let mut locations = Vec::with_capacity(cluster.members.len() + 2);
        locations.push(input.problem.locations()[previous].clone());
        locations.extend(
            cluster
                .members
                .iter()
                .map(|&member| input.problem.locations()[member].clone()),
        );
        locations.push(dummy);
        let subproblem = OptimizationProblem::new(
            locations,
            TimeOfDay::from_minutes(start_slot * TIME_SLOT_MINUTES as u16)
                .map_err(|error| SolverError::Failed(error.to_string()))?,
        );

        let dummy_index = cluster.members.len() + 1;
        let mut mapping = Vec::with_capacity(dummy_index);
        mapping.push(previous);
        mapping.extend(cluster.members.iter().copied());
        let mut rows = vec![vec![0_u32; dummy_index + 1]; dummy_index + 1];
        for from in 0..dummy_index {
            for to in 0..dummy_index {
                if from != to {
                    rows[from][to] = input
                        .matrix
                        .travel_minutes(mapping[from], mapping[to])
                        .ok_or(SolverError::MatrixSizeMismatch {
                            matrix: input.matrix.size(),
                            locations: input.problem.locations().len(),
                        })?;
                }
            }
            rows[from][dummy_index] = next_candidates
                .iter()
                .filter_map(|&next| input.matrix.travel_minutes(mapping[from], next))
                .min()
                .ok_or(SolverError::NoFeasibleRoute)?;
        }
        let submatrix =
            TravelTimeMatrix::new(rows).map_err(|error| SolverError::Failed(error.to_string()))?;
        let detailed = self.exact.solve_detailed(SolverInput {
            matrix: &submatrix,
            problem: &subproblem,
            cancellation: input.cancellation,
        })?;
        let order = detailed.solution.visit_order[1..detailed.solution.visit_order.len() - 1]
            .iter()
            .map(|&sub_index| cluster.members[sub_index - 1])
            .collect();
        Ok(SolvedCluster {
            order,
            generated_states: detailed.stats.generated_states,
            frontier_states: detailed.stats.frontier_states,
        })
    }
}

struct SolvedCluster {
    order: Vec<usize>,
    generated_states: usize,
    frontier_states: usize,
}

impl<
        C: ClusterStrategy,
        R: ClusterOrderStrategy,
        L: LocalImprovementStrategy,
        O: ObjectivePolicy,
        F: FrontierPolicy,
    > RouteSolver for ClusteredSolver<C, R, L, O, F>
{
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        self.solve_detailed(input).map(|result| result.solution)
    }

    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        let metrics = evaluate_solution(&self.exact.objective, input, solution)?;
        TimeOfDay::from_minutes(metrics.start_time_slot * TIME_SLOT_MINUTES as u16)
            .map_err(|error| SolverError::Failed(error.to_string()))
    }
}

fn validate_clusters(
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

fn validate_cluster_order(order: &[usize], cluster_count: usize) -> Result<(), SolverError> {
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

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
        Arc,
    };

    use crate::{
        api::OptimizeRouteRequest, cancellation::CancellationToken,
        schedule::calculate_schedule_from,
    };

    use super::*;

    fn problem(location_count: usize) -> OptimizationProblem {
        let locations: Vec<_> = (0..location_count)
            .map(|index| {
                serde_json::json!({
                    "id": index.to_string(),
                    "place_id": format!("place-{index}"),
                    "open_time": if index == 0 || index + 1 == location_count { "00:00" } else { "09:00" },
                    "close_time": "23:59",
                    "stay_minutes": if index == 0 || index + 1 == location_count { 0 } else { 10 }
                })
            })
            .collect();
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "clustered-solver-test",
            "locations": locations,
            "start_time": "09:00"
        }))
        .unwrap();
        request.try_into().unwrap()
    }

    fn directed_linear_matrix(size: usize) -> TravelTimeMatrix {
        let rows = (0..size)
            .map(|from| {
                (0..size)
                    .map(|to| {
                        if from == to {
                            0
                        } else if to == from + 1 {
                            10
                        } else if to > from {
                            30 + (to - from) as u32
                        } else {
                            200 + (from - to) as u32
                        }
                    })
                    .collect()
            })
            .collect();
        TravelTimeMatrix::new(rows).unwrap()
    }

    fn solve_clustered(
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
        max_cluster_size: usize,
    ) -> ClusteredSolveResult {
        ClusteredSolver::with_config(ClusteredSolverConfig::new(max_cluster_size).unwrap())
            .solve_detailed(SolverInput {
                matrix,
                problem,
                cancellation: &CancellationToken::new(),
            })
            .unwrap()
    }

    #[test]
    fn mst_cluster_order_is_deterministic_and_covers_each_cluster() {
        let problem = problem(7);
        let matrix = directed_linear_matrix(7);
        let clusters = vec![
            Cluster::new(vec![1, 2]),
            Cluster::new(vec![3, 4]),
            Cluster::new(vec![5]),
        ];
        let strategy = MstClusterOrderStrategy::default();
        let first = strategy.order(&clusters, &problem, &matrix).unwrap();
        let second = strategy.order(&clusters, &problem, &matrix).unwrap();

        assert_eq!(first, second);
        let mut covered = first;
        covered.sort_unstable();
        assert_eq!(covered, vec![0, 1, 2]);
    }

    #[test]
    fn large_route_contains_every_location_once_and_respects_cluster_limit() {
        let problem = problem(18);
        let matrix = directed_linear_matrix(18);
        let result = solve_clustered(&problem, &matrix, 5);

        assert_eq!(result.solution.visit_order.len(), 18);
        let mut locations = result.solution.visit_order.clone();
        locations.sort_unstable();
        assert_eq!(locations, (0..18).collect::<Vec<_>>());
        assert!(result.stats.cluster_sizes.iter().all(|&size| size <= 5));
        assert_eq!(result.stats.cluster_count, 4);
        assert_eq!(result.stats.cluster_strategy, "directed_nearest_neighbor");
        assert_eq!(result.stats.cluster_order_strategy, "greedy_bridge");
        assert_eq!(result.stats.cluster_order.len(), 4);
        assert_eq!(result.stats.cluster_details.len(), 4);
        assert!(result
            .stats
            .cluster_details
            .iter()
            .zip(&result.stats.cluster_order)
            .all(|(detail, order)| detail.cluster_index == *order));
        assert!(result.stats.cluster_details.iter().all(|detail| {
            detail.entry == detail.route.first().copied()
                && detail.exit == detail.route.last().copied()
                && detail.members.len() == detail.route.len()
                && detail.exact_generated_states > 0
                && detail.exact_frontier_states > 0
        }));
        assert!(result.stats.exact_generated_states > 0);
        assert!(result.stats.exact_frontier_states > 0);
        assert_eq!(result.stats.improvement_strategy, "boundary_swap");
        assert!(result.stats.improvement_operations.swap);
        assert!(!result.stats.improvement_operations.relocate);
        assert!(!result.stats.improvement_operations.two_opt);
    }

    #[test]
    fn merged_route_is_feasible_with_directed_times_and_time_windows() {
        let problem = problem(12);
        let matrix = directed_linear_matrix(12);
        let result = solve_clustered(&problem, &matrix, 4);

        assert_eq!(result.solution.visit_order, (0..12).collect::<Vec<_>>());
        let evaluated = evaluate_solution(
            &DefaultObjectivePolicy,
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            },
            &result.solution,
        )
        .unwrap();
        assert_eq!(evaluated, result.metrics);
    }

    #[test]
    fn restrictive_time_windows_are_preserved_across_cluster_merge() {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "clustered-time-window-test",
            "locations": [
                {"id":"0","place_id":"p0","open_time":"00:00","close_time":"23:59","stay_minutes":0},
                {"id":"1","place_id":"p1","open_time":"09:00","close_time":"10:00","stay_minutes":0},
                {"id":"2","place_id":"p2","open_time":"09:00","close_time":"11:00","stay_minutes":0},
                {"id":"3","place_id":"p3","open_time":"09:00","close_time":"12:00","stay_minutes":0},
                {"id":"4","place_id":"p4","open_time":"00:00","close_time":"23:59","stay_minutes":0}
            ],
            "start_time": "09:00"
        }))
        .unwrap();
        let problem: OptimizationProblem = request.try_into().unwrap();
        let matrix = TravelTimeMatrix::new(
            (0..5)
                .map(|from| (0..5).map(|to| if from == to { 0 } else { 10 }).collect())
                .collect(),
        )
        .unwrap();
        let result = solve_clustered(&problem, &matrix, 2);

        assert_eq!(result.metrics.start_time_slot, 59); // 09:50
        let start_time =
            TimeOfDay::from_minutes(result.metrics.start_time_slot * TIME_SLOT_MINUTES as u16)
                .unwrap();
        let plan =
            calculate_schedule_from(&problem, &matrix, &result.solution, start_time).unwrap();
        let constrained = plan
            .stops
            .iter()
            .find(|stop| stop.location_index == 1)
            .unwrap();
        assert!(constrained.departure_time.unwrap().minutes() <= 10 * 60);
    }

    #[test]
    fn local_improvement_never_worsens_the_shared_objective() {
        let problem = problem(14);
        let matrix = directed_linear_matrix(14);
        let result = solve_clustered(&problem, &matrix, 4);

        assert_ne!(
            DefaultObjectivePolicy.compare(&result.metrics, &result.metrics_before_improvement),
            Ordering::Greater
        );
    }

    #[test]
    fn small_problem_can_be_compared_with_exact_solution() {
        let problem = problem(8);
        let matrix = directed_linear_matrix(8);
        let cancellation = CancellationToken::new();
        let exact = ExactBitDpSolver::default()
            .solve_detailed(SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &cancellation,
            })
            .unwrap();
        let clustered = solve_clustered(&problem, &matrix, 3);

        assert_eq!(clustered.solution, exact.solution);
        assert_eq!(clustered.metrics, exact.metrics);
    }

    #[derive(Clone)]
    struct CountingSolver {
        calls: Arc<AtomicUsize>,
    }

    impl RouteSolver for CountingSolver {
        fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(SolverSolution {
                visit_order: (0..input.problem.locations().len()).collect(),
            })
        }
    }

    #[test]
    fn automatic_selection_uses_exact_at_limit_and_clustered_above_it() {
        let exact_calls = Arc::new(AtomicUsize::new(0));
        let clustered_calls = Arc::new(AtomicUsize::new(0));
        let solver = AutomaticRouteSolver::new(
            CountingSolver {
                calls: Arc::clone(&exact_calls),
            },
            CountingSolver {
                calls: Arc::clone(&clustered_calls),
            },
            ThresholdSolverSelectionPolicy::new(3).unwrap(),
        );

        for size in [3, 4] {
            let problem = problem(size);
            let matrix = directed_linear_matrix(size);
            solver
                .solve(SolverInput {
                    matrix: &matrix,
                    problem: &problem,
                    cancellation: &CancellationToken::new(),
                })
                .unwrap();
        }
        assert_eq!(exact_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(clustered_calls.load(AtomicOrdering::Relaxed), 1);
    }
}
