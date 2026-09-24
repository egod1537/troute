mod config;
mod diagnostics;
mod runner;
mod selector;

pub use config::*;
pub use diagnostics::SaInitializer;
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
            total_budget: Duration::from_millis(75),
            sa_chunk: Duration::from_millis(10),
            deadline_safety_margin: Duration::from_millis(2),
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
            start_policy: crate::domain::StartPolicy::Latest,
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
                    start_policy: crate::domain::StartPolicy::Latest,
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
                    start_policy: crate::domain::StartPolicy::Latest,
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
                    start_policy: crate::domain::StartPolicy::Latest,
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
    fn selector_applies_earliest_and_fixed_start_policies() {
        let evaluator = DefaultObjectivePolicy;
        let metrics = |policy, start_time_slot, finish_time_slot| SolutionMetrics {
            start_policy: policy,
            start_time_slot,
            finish_time_slot,
            travel_minutes: 10,
            wait_minutes: 0,
            score: 10,
        };

        assert!(evaluator
            .compare(
                &metrics(crate::domain::StartPolicy::Earliest, 10, 30),
                &metrics(crate::domain::StartPolicy::Earliest, 11, 20),
            )
            .is_lt());
        assert!(evaluator
            .compare(
                &metrics(crate::domain::StartPolicy::Fixed, 10, 20),
                &metrics(crate::domain::StartPolicy::Fixed, 10, 21),
            )
            .is_lt());
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
    fn exact_success_terminates_before_baselines_and_sa() {
        let mut config = test_config();
        config.sa_seeds = vec![7, 11];
        let solver = SolverOrchestrator::new(config).unwrap();
        let problem = problem(5);
        let matrix = matrix(5);
        let cancellation = CancellationToken::new();
        let first = solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();
        assert_eq!(first.selected_strategy, "exact_bit_dp");
        assert_eq!(first.candidates.len(), 1);
        assert_eq!(first.sa_run_count, 0);
        assert_eq!(first.termination_reason, "exact_optimum");
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

    #[test]
    fn large_problem_runs_all_initializers_then_warm_starts_until_deadline() {
        let mut config = test_config();
        config.exact_limit = 3;
        config.total_budget = Duration::from_millis(60);
        config.sa_chunk = Duration::from_millis(4);
        config.deadline_safety_margin = Duration::from_millis(2);
        config.sa_config.iteration_limit = Some(25);
        let solver = SolverOrchestrator::new(config).unwrap();
        let problem = problem(6);
        let matrix = matrix(6);
        let cancellation = CancellationToken::new();
        let result = solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();

        assert!(result.sa_run_count >= 5);
        for prefix in [
            "sa_greedy_run_1_",
            "sa_mst_run_2_",
            "sa_christofides_run_3_",
            "sa_clustered_run_4_",
            "sa_warm_start_run_5_",
        ] {
            assert!(result
                .candidates
                .iter()
                .any(|candidate| candidate.strategy.starts_with(prefix)));
        }
        let warm = result
            .candidates
            .iter()
            .find(|candidate| candidate.strategy.starts_with("sa_warm_start_run_5_"))
            .unwrap();
        let best_before_warm = result
            .candidates
            .iter()
            .filter(|candidate| {
                !candidate.strategy.starts_with("sa_")
                    || (1..=4).any(|run| candidate.strategy.contains(&format!("_run_{run}_")))
            })
            .filter(|candidate| candidate.feasible)
            .min_by(|left, right| {
                DefaultObjectivePolicy.compare(
                    left.objective_score.as_ref().unwrap(),
                    right.objective_score.as_ref().unwrap(),
                )
            })
            .unwrap();
        let best_route_ids = best_before_warm
            .route
            .as_ref()
            .unwrap()
            .visit_order
            .iter()
            .map(|&index| problem.locations()[index].id().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(warm.metadata.initial_route.as_ref(), Some(&best_route_ids));
        assert_eq!(result.termination_reason, "deadline");
        assert!(result
            .candidates
            .iter()
            .any(|candidate| candidate.strategy == "greedy"));
    }

    #[test]
    fn seed_sequence_is_deterministic_and_objective_never_selects_a_worse_result() {
        let mut config = test_config();
        config.exact_limit = 3;
        config.total_budget = Duration::from_millis(45);
        config.sa_chunk = Duration::from_millis(3);
        config.deadline_safety_margin = Duration::from_millis(2);
        config.sa_config.iteration_limit = Some(20);
        config.sa_seeds = vec![1234];
        let problem = problem(6);
        let matrix = matrix(6);
        let solve = || {
            let cancellation = CancellationToken::new();
            SolverOrchestrator::new(config.clone())
                .unwrap()
                .solve_detailed(input(&problem, &matrix, &cancellation))
                .unwrap()
        };
        let first = solve();
        let second = solve();
        let first_four = |result: &OrchestratorSolveResult| {
            (1..=4)
                .map(|run| {
                    result
                        .candidates
                        .iter()
                        .find(|candidate| candidate.strategy.contains(&format!("_run_{run}_")))
                        .and_then(|candidate| candidate.metadata.seed)
                        .unwrap()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(first_four(&first), first_four(&second));

        let selected = first.candidates[0].objective_score.unwrap();
        assert!(first
            .candidates
            .iter()
            .filter_map(|candidate| candidate.objective_score)
            .all(|metrics| !DefaultObjectivePolicy.compare(&selected, &metrics).is_gt()));
    }

    #[test]
    fn cancellation_preserves_a_baseline_feasible_solution() {
        let mut config = test_config();
        config.exact_limit = 3;
        config.total_budget = Duration::from_millis(500);
        config.sa_chunk = Duration::from_millis(100);
        config.deadline_safety_margin = Duration::from_millis(2);
        config.sa_config.iteration_limit = None;
        let solver = SolverOrchestrator::new(config).unwrap();
        let problem = problem(8);
        let matrix = matrix(8);
        let cancellation = CancellationToken::new();
        let canceller = cancellation.clone();
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            canceller.cancel();
        });
        let result = solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();
        handle.join().unwrap();

        assert_eq!(result.termination_reason, "cancellation");
        assert!(result.candidates[0].feasible);
    }
}
