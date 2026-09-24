use std::sync::Arc;

use crate::solver::{
    AutoPerfectMatching, AverageSymmetricDistance, ChristofidesInitialRoute, ClusterDiagnostic,
    ClusteredInitialRoute, ClusteredSolver, ClusteredSolverConfig, DefaultObjectivePolicy,
    ExactBitDpSolver, GreedyInitialRoute, InitialRouteStrategy, MatchingPairDiagnostic,
    MixedNeighborhoodStrategy, MstDoubleTreeInitialRoute, MstEdgeDiagnostic,
    SimulatedAnnealingConfig, SimulatedAnnealingSolver, SolverCandidateMetadata, SolverError,
    SolverInput,
};

use super::{OrchestratedStrategy, SolverOrchestratorConfig, StrategyExecution};

pub(super) fn built_in_strategies(
    config: &SolverOrchestratorConfig,
) -> Result<Vec<Arc<dyn OrchestratedStrategy>>, SolverError> {
    let cluster_config = ClusteredSolverConfig::new(config.max_cluster_size)?;
    let matching = AutoPerfectMatching::new(config.matching)?;
    let mut strategies: Vec<Arc<dyn OrchestratedStrategy>> = vec![
        Arc::new(ExactStrategy {
            exact_limit: config.exact_limit,
            solver: ExactBitDpSolver::default(),
        }),
        Arc::new(ClusteredStrategy {
            solver: ClusteredSolver::with_config(cluster_config),
        }),
        Arc::new(MstDoubleTreeStrategy {
            generator: MstDoubleTreeInitialRoute::default(),
        }),
        Arc::new(ChristofidesStrategy {
            generator: ChristofidesInitialRoute::new(AverageSymmetricDistance, matching),
        }),
    ];

    for &seed in &config.sa_seeds {
        let sa_config = SimulatedAnnealingConfig {
            seed: Some(seed),
            ..config.sa_config
        };
        strategies.push(Arc::new(AnnealingStrategy::new(
            format!("sa_greedy_seed_{seed}"),
            "greedy",
            seed,
            sa_config,
            GreedyInitialRoute,
        )?));
        strategies.push(Arc::new(AnnealingStrategy::new(
            format!("sa_mst_seed_{seed}"),
            "mst_double_tree",
            seed,
            sa_config,
            MstDoubleTreeInitialRoute::default(),
        )?));
        strategies.push(Arc::new(AnnealingStrategy::new(
            format!("sa_christofides_seed_{seed}"),
            "christofides",
            seed,
            sa_config,
            ChristofidesInitialRoute::new(AverageSymmetricDistance, matching),
        )?));
        strategies.push(Arc::new(AnnealingStrategy::new(
            format!("sa_clustered_seed_{seed}"),
            "clustered",
            seed,
            sa_config,
            ClusteredInitialRoute::new(ClusteredSolver::with_config(cluster_config)),
        )?));
    }
    Ok(strategies)
}

struct ExactStrategy {
    exact_limit: usize,
    solver: ExactBitDpSolver,
}

impl OrchestratedStrategy for ExactStrategy {
    fn name(&self) -> &str {
        "exact_bit_dp"
    }

    fn is_applicable(&self, location_count: usize) -> bool {
        location_count <= self.exact_limit
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        let result = self.solver.solve_detailed(input)?;
        Ok(StrategyExecution {
            solution: result.solution,
            metadata: SolverCandidateMetadata {
                state_count: Some(result.stats.generated_states),
                frontier_state_count: Some(result.stats.frontier_states),
                frontier_cell_count: Some(result.stats.frontier_cells),
                ..SolverCandidateMetadata::default()
            },
        })
    }
}

struct ClusteredStrategy {
    solver: ClusteredSolver,
}

impl OrchestratedStrategy for ClusteredStrategy {
    fn name(&self) -> &str {
        "clustered"
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        let problem = input.problem;
        let result = self.solver.solve_detailed(input)?;
        let location_id = |index: usize| problem.locations()[index].id().to_owned();
        Ok(StrategyExecution {
            solution: result.solution,
            metadata: SolverCandidateMetadata {
                state_count: Some(result.stats.exact_generated_states),
                frontier_state_count: Some(result.stats.exact_frontier_states),
                cluster_count: Some(result.stats.cluster_count),
                cluster_sizes: Some(result.stats.cluster_sizes),
                cluster_strategy: Some(result.stats.cluster_strategy),
                cluster_order_strategy: Some(result.stats.cluster_order_strategy),
                cluster_order: Some(
                    result
                        .stats
                        .cluster_order
                        .iter()
                        .map(|index| index + 1)
                        .collect(),
                ),
                cluster_details: Some(
                    result
                        .stats
                        .cluster_details
                        .iter()
                        .map(|cluster| ClusterDiagnostic {
                            cluster: cluster.cluster_index + 1,
                            members: cluster
                                .members
                                .iter()
                                .map(|&index| location_id(index))
                                .collect(),
                            route: cluster
                                .route
                                .iter()
                                .map(|&index| location_id(index))
                                .collect(),
                            entry: cluster.entry.map(location_id),
                            exit: cluster.exit.map(location_id),
                            state_count: cluster.exact_generated_states,
                            frontier_state_count: cluster.exact_frontier_states,
                        })
                        .collect(),
                ),
                score_before_improvement: Some(result.metrics_before_improvement.score),
                score_after_improvement: Some(result.metrics.score),
                improvement_strategy: Some(result.stats.improvement_strategy),
                swap_enabled: Some(result.stats.improvement_operations.swap),
                relocate_enabled: Some(result.stats.improvement_operations.relocate),
                two_opt_enabled: Some(result.stats.improvement_operations.two_opt),
                improved_moves: Some(result.stats.accepted_local_moves as u64),
                ..SolverCandidateMetadata::default()
            },
        })
    }
}

struct MstDoubleTreeStrategy {
    generator: MstDoubleTreeInitialRoute,
}

impl OrchestratedStrategy for MstDoubleTreeStrategy {
    fn name(&self) -> &str {
        "mst_double_tree"
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        let location_ids: Vec<_> = input
            .problem
            .locations()
            .iter()
            .map(|location| location.id().to_owned())
            .collect();
        let result = self.generator.generate_detailed(input)?;
        let location_id = |index: usize| location_ids[index].clone();
        let mst_cost = result.mst_edges.iter().map(|edge| edge.distance).sum();
        let metadata = SolverCandidateMetadata {
            symmetric_distance_strategy: Some(result.symmetric_distance_strategy),
            mst_cost: Some(mst_cost),
            mst_edge_count: Some(result.mst_edges.len()),
            mst_edges: Some(
                result
                    .mst_edges
                    .iter()
                    .map(|edge| MstEdgeDiagnostic {
                        from: location_id(edge.from),
                        to: location_id(edge.to),
                        distance: edge.distance,
                    })
                    .collect(),
            ),
            euler_tour: Some(
                result
                    .euler_tour
                    .iter()
                    .map(|&index| location_id(index))
                    .collect(),
            ),
            shortcut_route: Some(
                result
                    .solution
                    .visit_order
                    .iter()
                    .map(|&index| location_id(index))
                    .collect(),
            ),
            seed: Some(0),
            ..SolverCandidateMetadata::default()
        };
        Ok(StrategyExecution {
            solution: result.solution,
            metadata,
        })
    }
}

struct ChristofidesStrategy {
    generator: ChristofidesInitialRoute<AverageSymmetricDistance, AutoPerfectMatching>,
}

impl OrchestratedStrategy for ChristofidesStrategy {
    fn name(&self) -> &str {
        "christofides"
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        let location_ids: Vec<_> = input
            .problem
            .locations()
            .iter()
            .map(|location| location.id().to_owned())
            .collect();
        let result = self.generator.generate_detailed(input)?;
        let location_id = |index: usize| location_ids[index].clone();
        let mst_cost = result.mst_edges.iter().map(|edge| edge.distance).sum();
        let matching_cost = result.matching_edges.iter().map(|edge| edge.distance).sum();
        let metadata = SolverCandidateMetadata {
            symmetric_distance_strategy: Some(result.symmetric_distance_strategy),
            mst_cost: Some(mst_cost),
            mst_edge_count: Some(result.mst_edges.len()),
            mst_edges: Some(
                result
                    .mst_edges
                    .iter()
                    .map(|edge| MstEdgeDiagnostic {
                        from: location_id(edge.from),
                        to: location_id(edge.to),
                        distance: edge.distance,
                    })
                    .collect(),
            ),
            odd_vertices: Some(
                result
                    .odd_vertices
                    .iter()
                    .map(|&index| location_id(index))
                    .collect(),
            ),
            odd_vertex_count: Some(result.odd_vertices.len()),
            matching_strategy: Some(result.matching_strategy),
            matching_cost: Some(matching_cost),
            matching_pairs: Some(
                result
                    .matching_edges
                    .iter()
                    .map(|edge| MatchingPairDiagnostic {
                        left: location_id(edge.left),
                        right: location_id(edge.right),
                        distance: edge.distance,
                    })
                    .collect(),
            ),
            euler_tour: Some(
                result
                    .euler_tour
                    .iter()
                    .map(|&index| location_id(index))
                    .collect(),
            ),
            shortcut_route: Some(
                result
                    .solution
                    .visit_order
                    .iter()
                    .map(|&index| location_id(index))
                    .collect(),
            ),
            seed: Some(0),
            ..SolverCandidateMetadata::default()
        };
        Ok(StrategyExecution {
            solution: result.solution,
            metadata,
        })
    }
}

struct AnnealingStrategy<I> {
    name: String,
    initial_strategy: &'static str,
    seed: u64,
    solver: SimulatedAnnealingSolver<I, MixedNeighborhoodStrategy, DefaultObjectivePolicy>,
}

impl<I> AnnealingStrategy<I> {
    fn new(
        name: String,
        initial_strategy: &'static str,
        seed: u64,
        config: SimulatedAnnealingConfig,
        initial: I,
    ) -> Result<Self, SolverError> {
        Ok(Self {
            name,
            initial_strategy,
            seed,
            solver: SimulatedAnnealingSolver::new(
                config,
                initial,
                MixedNeighborhoodStrategy,
                DefaultObjectivePolicy,
            )?,
        })
    }
}

impl<I: InitialRouteStrategy> OrchestratedStrategy for AnnealingStrategy<I> {
    fn name(&self) -> &str {
        &self.name
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        let location_ids: Vec<_> = input
            .problem
            .locations()
            .iter()
            .map(|location| location.id().to_owned())
            .collect();
        let result = self.solver.solve_detailed(input)?;
        let location_id = |index: usize| location_ids[index].clone();
        Ok(StrategyExecution {
            solution: result.solution.clone(),
            metadata: SolverCandidateMetadata {
                initial_strategy: Some(self.initial_strategy.to_owned()),
                initial_route: Some(
                    result
                        .initial_solution
                        .visit_order
                        .iter()
                        .map(|&index| location_id(index))
                        .collect(),
                ),
                final_route: Some(
                    result
                        .solution
                        .visit_order
                        .iter()
                        .map(|&index| location_id(index))
                        .collect(),
                ),
                initial_score: result.initial_metrics.map(|metrics| metrics.score),
                final_score: Some(result.metrics.score),
                initial_temperature: Some(result.stats.initial_temperature.to_string()),
                final_temperature: Some(result.stats.final_temperature.to_string()),
                cooling_rate: Some(result.stats.cooling_rate.to_string()),
                swap_move_count: Some(result.stats.swap_moves),
                relocate_move_count: Some(result.stats.relocate_moves),
                two_opt_move_count: Some(result.stats.two_opt_moves),
                accepted_worse_moves: Some(result.stats.accepted_worse_moves),
                infeasible_candidates: Some(result.stats.infeasible_candidates),
                accepted_infeasible_moves: Some(result.stats.accepted_infeasible_moves),
                best_feasible: Some(true),
                iteration_count: Some(result.stats.iterations),
                accepted_moves: Some(result.stats.accepted_moves),
                improved_moves: Some(result.stats.improved_moves),
                seed: Some(self.seed),
                ..SolverCandidateMetadata::default()
            },
        })
    }
}
