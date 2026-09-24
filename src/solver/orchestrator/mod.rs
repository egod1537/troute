mod config;
mod diagnostics;
mod runner;
mod selector;

pub use config::*;
pub use runner::*;
pub use selector::*;

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering as TestOrdering},
            Arc,
        },
        thread,
        time::Duration,
    };

    use super::*;
    use crate::{
        api::OptimizeRouteRequest,
        cancellation::CancellationToken,
        domain::OptimizationProblem,
        matrix::TravelTimeMatrix,
        solver::{
            DefaultObjectivePolicy, ObjectiveEvaluator, SimulatedAnnealingConfig, SolutionMetrics,
            SolverCandidate, SolverCandidateMetadata, SolverError, SolverInput, SolverSolution,
        },
    };

    #[derive(Clone)]
    struct FixedStrategy {
        name: &'static str,
        route: Option<Vec<usize>>,
        fail: bool,
        wait_for_cancellation: bool,
    }

    struct ConcurrencyStrategy {
        name: String,
        active: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    impl OrchestratedStrategy for ConcurrencyStrategy {
        fn name(&self) -> &str {
            &self.name
        }

        fn execute(&self, _input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
            let active = self.active.fetch_add(1, TestOrdering::SeqCst) + 1;
            self.peak.fetch_max(active, TestOrdering::SeqCst);
            thread::sleep(Duration::from_millis(15));
            self.active.fetch_sub(1, TestOrdering::SeqCst);
            Ok(StrategyExecution {
                solution: SolverSolution {
                    visit_order: vec![0, 1],
                },
                metadata: SolverCandidateMetadata::default(),
            })
        }
    }

    impl OrchestratedStrategy for FixedStrategy {
        fn name(&self) -> &str {
            self.name
        }

        fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
            if self.wait_for_cancellation {
                while !input.cancellation.is_cancelled() {
                    thread::sleep(Duration::from_millis(1));
                }
                return Err(SolverError::Cancelled);
            }
            if self.fail {
                return Err(SolverError::Failed("intentional failure".to_owned()));
            }
            Ok(StrategyExecution {
                solution: SolverSolution {
                    visit_order: self.route.clone().unwrap(),
                },
                metadata: SolverCandidateMetadata::default(),
            })
        }
    }

    fn problem(location_count: usize) -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "orchestrator-test",
            "locations": (0..location_count).map(|index| serde_json::json!({
                "id": index.to_string(),
                "place_id": format!("place-{index}"),
                "open_time": "00:00",
                "close_time": "23:50",
                "stay_minutes": 0
            })).collect::<Vec<_>>(),
            "start_time": "09:00"
        }))
        .unwrap();
        request.try_into().unwrap()
    }

    fn matrix(size: usize) -> TravelTimeMatrix {
        TravelTimeMatrix::new(
            (0..size)
                .map(|from| {
                    (0..size)
                        .map(|to| {
                            if from == to {
                                0
                            } else if to == from + 1 {
                                10
                            } else {
                                50
                            }
                        })
                        .collect()
                })
                .collect(),
        )
        .unwrap()
    }

    fn test_config() -> SolverOrchestratorConfig {
        SolverOrchestratorConfig {
            max_concurrency: 2,
            strategy_timeout: Duration::from_secs(2),
            sa_config: SimulatedAnnealingConfig {
                iteration_limit: Some(50),
                seed: Some(42),
                ..SimulatedAnnealingConfig::default()
            },
            ..SolverOrchestratorConfig::default()
        }
    }

    fn input<'a>(
        problem: &'a OptimizationProblem,
        matrix: &'a TravelTimeMatrix,
        cancellation: &'a CancellationToken,
    ) -> SolverInput<'a> {
        SolverInput {
            matrix,
            problem,
            cancellation,
        }
    }

    #[test]
    fn selects_best_candidate_and_continues_after_failure() {
        let strategies: Vec<Arc<dyn OrchestratedStrategy>> = vec![
            Arc::new(FixedStrategy {
                name: "failure",
                route: None,
                fail: true,
                wait_for_cancellation: false,
            }),
            Arc::new(FixedStrategy {
                name: "heuristic",
                route: Some(vec![0, 2, 1, 3]),
                fail: false,
                wait_for_cancellation: false,
            }),
            Arc::new(FixedStrategy {
                name: "exact_bit_dp",
                route: Some(vec![0, 1, 2, 3]),
                fail: false,
                wait_for_cancellation: false,
            }),
        ];
        let solver =
            SolverOrchestrator::with_strategies(test_config(), DefaultObjectivePolicy, strategies)
                .unwrap();
        let problem = problem(4);
        let matrix = matrix(4);
        let cancellation = CancellationToken::new();
        let result = solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();

        assert_eq!(result.selected_strategy, "exact_bit_dp");
        assert_eq!(result.solution.visit_order, vec![0, 1, 2, 3]);
        assert_eq!(result.candidates.len(), 3);
        assert!(result
            .candidates
            .iter()
            .find(|candidate| candidate.strategy == "failure")
            .unwrap()
            .metadata
            .error
            .is_some());
    }

    #[test]
    fn stable_tie_break_keeps_registration_order() {
        let metrics = SolutionMetrics {
            start_time_slot: 10,
            finish_time_slot: 20,
            travel_minutes: 30,
            wait_minutes: 40,
            score: 70,
        };
        let candidate = |strategy: &str| SolverCandidate {
            strategy: strategy.to_owned(),
            route: Some(SolverSolution {
                visit_order: vec![0, 1],
            }),
            feasible: true,
            objective_score: Some(metrics),
            elapsed: Duration::ZERO,
            metadata: SolverCandidateMetadata::default(),
        };
        let mut candidates = vec![candidate("first"), candidate("second")];
        let evaluator = DefaultObjectivePolicy;
        let selector = CandidateSelector::new(&evaluator);
        selector.rank(&mut candidates);
        assert_eq!(selector.select(&candidates).unwrap().strategy, "first");
    }

    #[test]
    fn selector_uses_latest_start_finish_travel_then_wait() {
        let candidate = |strategy: &str, metrics: SolutionMetrics| SolverCandidate {
            strategy: strategy.to_owned(),
            route: Some(SolverSolution {
                visit_order: vec![0, 1],
            }),
            feasible: true,
            objective_score: Some(metrics),
            elapsed: Duration::ZERO,
            metadata: SolverCandidateMetadata::default(),
        };
        let mut candidates = vec![
            candidate(
                "more_wait",
                SolutionMetrics {
                    start_time_slot: 20,
                    finish_time_slot: 30,
                    travel_minutes: 10,
                    wait_minutes: 2,
                    score: 12,
                },
            ),
            candidate(
                "less_travel",
                SolutionMetrics {
                    start_time_slot: 20,
                    finish_time_slot: 30,
                    travel_minutes: 9,
                    wait_minutes: 100,
                    score: 109,
                },
            ),
            candidate(
                "earlier_start",
                SolutionMetrics {
                    start_time_slot: 19,
                    finish_time_slot: 20,
                    travel_minutes: 1,
                    wait_minutes: 0,
                    score: 1,
                },
            ),
        ];
        let evaluator = DefaultObjectivePolicy;
        CandidateSelector::new(&evaluator).rank(&mut candidates);
        assert_eq!(candidates[0].strategy, "less_travel");
        assert_eq!(candidates[1].strategy, "more_wait");
        assert_eq!(candidates[2].strategy, "earlier_start");
    }

    #[test]
    fn timeout_is_recorded_without_stopping_other_strategies() {
        let mut config = test_config();
        config.strategy_timeout = Duration::from_millis(10);
        let strategies: Vec<Arc<dyn OrchestratedStrategy>> = vec![
            Arc::new(FixedStrategy {
                name: "slow",
                route: None,
                fail: false,
                wait_for_cancellation: true,
            }),
            Arc::new(FixedStrategy {
                name: "fast",
                route: Some(vec![0, 1, 2]),
                fail: false,
                wait_for_cancellation: false,
            }),
        ];
        let solver =
            SolverOrchestrator::with_strategies(config, DefaultObjectivePolicy, strategies)
                .unwrap();
        let problem = problem(3);
        let matrix = matrix(3);
        let cancellation = CancellationToken::new();
        let result = solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();

        assert_eq!(result.selected_strategy, "fast");
        let timed_out = result
            .candidates
            .iter()
            .find(|candidate| candidate.strategy == "slow")
            .unwrap();
        assert!(timed_out.metadata.timed_out);
    }

    #[test]
    fn max_concurrency_bounds_simultaneous_strategies() {
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let strategies: Vec<Arc<dyn OrchestratedStrategy>> = (0..4)
            .map(|index| {
                Arc::new(ConcurrencyStrategy {
                    name: format!("parallel-{index}"),
                    active: Arc::clone(&active),
                    peak: Arc::clone(&peak),
                }) as Arc<dyn OrchestratedStrategy>
            })
            .collect();
        let mut config = test_config();
        config.max_concurrency = 2;
        let solver =
            SolverOrchestrator::with_strategies(config, DefaultObjectivePolicy, strategies)
                .unwrap();
        let problem = problem(2);
        let matrix = matrix(2);
        let cancellation = CancellationToken::new();

        solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();
        assert_eq!(peak.load(TestOrdering::SeqCst), 2);
    }

    #[test]
    fn reports_no_feasible_candidate() {
        let strategies: Vec<Arc<dyn OrchestratedStrategy>> = vec![Arc::new(FixedStrategy {
            name: "invalid",
            route: Some(vec![0, 0, 2]),
            fail: false,
            wait_for_cancellation: false,
        })];
        let solver =
            SolverOrchestrator::with_strategies(test_config(), DefaultObjectivePolicy, strategies)
                .unwrap();
        let problem = problem(3);
        let matrix = matrix(3);
        let cancellation = CancellationToken::new();
        assert!(matches!(
            solver.solve_detailed(input(&problem, &matrix, &cancellation)),
            Err(SolverError::NoFeasibleRoute)
        ));
    }

    #[test]
    fn built_ins_include_exact_and_deterministic_sa_multistart() {
        let mut config = test_config();
        config.sa_seeds = vec![7, 11];
        let solver = SolverOrchestrator::new(config).unwrap();
        let problem = problem(5);
        let matrix = matrix(5);
        let cancellation = CancellationToken::new();
        let first = solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();
        let second = solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();

        assert!(first
            .candidates
            .iter()
            .any(|candidate| candidate.strategy == "exact_bit_dp"));
        assert_eq!(
            first
                .candidates
                .iter()
                .filter(|candidate| candidate.strategy.starts_with("sa_"))
                .count(),
            8
        );
        let first_sa: Vec<_> = first
            .candidates
            .iter()
            .filter(|candidate| candidate.strategy.starts_with("sa_"))
            .map(|candidate| {
                (
                    candidate.strategy.clone(),
                    candidate.route.clone(),
                    candidate.objective_score,
                )
            })
            .collect();
        let second_sa: Vec<_> = second
            .candidates
            .iter()
            .filter(|candidate| candidate.strategy.starts_with("sa_"))
            .map(|candidate| {
                (
                    candidate.strategy.clone(),
                    candidate.route.clone(),
                    candidate.objective_score,
                )
            })
            .collect();
        assert_eq!(first_sa, second_sa);
        for candidate in first
            .candidates
            .iter()
            .filter(|candidate| candidate.strategy.starts_with("sa_"))
        {
            let metadata = &candidate.metadata;
            assert!(metadata.initial_strategy.is_some());
            assert_eq!(metadata.initial_route.as_ref().unwrap().len(), 5);
            assert_eq!(
                metadata.final_route.as_ref().unwrap(),
                &candidate
                    .route
                    .as_ref()
                    .unwrap()
                    .visit_order
                    .iter()
                    .map(|&index| problem.locations()[index].id().to_owned())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                metadata.final_score,
                candidate.objective_score.map(|score| score.score)
            );
            assert!(metadata.initial_temperature.is_some());
            assert!(metadata.final_temperature.is_some());
            assert_eq!(metadata.cooling_rate.as_deref(), Some("0.995"));
            assert_eq!(
                metadata.swap_move_count.unwrap()
                    + metadata.relocate_move_count.unwrap()
                    + metadata.two_opt_move_count.unwrap(),
                metadata.iteration_count.unwrap()
            );
            assert_eq!(metadata.best_feasible, Some(true));
        }

        let mst = first
            .candidates
            .iter()
            .find(|candidate| candidate.strategy == "mst_double_tree")
            .unwrap();
        assert_eq!(
            mst.metadata.symmetric_distance_strategy.as_deref(),
            Some("average_bidirectional")
        );
        assert_eq!(mst.metadata.mst_edge_count, Some(4));
        assert_eq!(mst.metadata.mst_edges.as_ref().unwrap().len(), 4);
        assert_eq!(mst.metadata.euler_tour.as_ref().unwrap().len(), 9);
        assert_eq!(
            mst.metadata.shortcut_route.as_ref().unwrap(),
            &mst.route
                .as_ref()
                .unwrap()
                .visit_order
                .iter()
                .map(|&index| problem.locations()[index].id().to_owned())
                .collect::<Vec<_>>()
        );

        let christofides = first
            .candidates
            .iter()
            .find(|candidate| candidate.strategy == "christofides")
            .unwrap();
        assert_eq!(christofides.metadata.mst_edge_count, Some(4));
        assert_eq!(
            christofides.metadata.matching_strategy.as_deref(),
            Some("bit_dp")
        );
        assert_eq!(
            christofides.metadata.odd_vertex_count,
            christofides.metadata.odd_vertices.as_ref().map(Vec::len)
        );
        assert_eq!(
            christofides.metadata.matching_pairs.as_ref().unwrap().len() * 2,
            christofides.metadata.odd_vertex_count.unwrap()
        );
        assert_eq!(
            christofides.metadata.shortcut_route.as_ref().unwrap(),
            &christofides
                .route
                .as_ref()
                .unwrap()
                .visit_order
                .iter()
                .map(|&index| problem.locations()[index].id().to_owned())
                .collect::<Vec<_>>()
        );

        let exact = first
            .candidates
            .iter()
            .find(|candidate| candidate.strategy == "exact_bit_dp")
            .unwrap()
            .objective_score
            .unwrap();
        let selected = first.candidates[0].objective_score.unwrap();
        assert!(!DefaultObjectivePolicy.compare(&exact, &selected).is_gt());
        assert!(!DefaultObjectivePolicy.compare(&selected, &exact).is_gt());
    }

    #[test]
    fn omits_exact_above_configured_limit() {
        let mut config = test_config();
        config.exact_limit = 3;
        let solver = SolverOrchestrator::new(config).unwrap();
        let problem = problem(4);
        let matrix = matrix(4);
        let cancellation = CancellationToken::new();
        let result = solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();
        assert!(!result
            .candidates
            .iter()
            .any(|candidate| candidate.strategy == "exact_bit_dp"));
    }
}
