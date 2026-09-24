use crate::domain::TimeOfDay;

use crate::solver::{
    evaluate_solution, minutes_to_slot_ceil, transition_time, DefaultObjectivePolicy,
    ExactBitDpSolver, FrontierPolicy, ObjectivePolicy, RouteSolver, SolutionMetrics, SolverError,
    SolverInput, SolverSolution, TimeCostFrontierPolicy,
};

use super::{
    solve_cluster, validate_cluster_order, validate_clusters, BoundarySwapLocalImprovement,
    ClusterOrderStrategy, ClusterStrategy, ClusteredSolverConfig, GreedyClusterOrderStrategy,
    LocalImprovementOperations, LocalImprovementStrategy, TravelTimeClusterStrategy,
    MAX_CLUSTER_MEMBERS_WITH_ANCHORS,
};

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
            minutes_to_slot_ceil(u32::from(input.problem.start_time().minutes())) as u16;
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
            let solved = solve_cluster(
                &self.exact,
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
        crate::solver::selected_start_time(input.problem, &metrics)
    }
}
