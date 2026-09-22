use std::{
    any::Any,
    cmp::Ordering,
    collections::{HashSet, VecDeque},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU8, Ordering as AtomicOrdering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use rand::{rngs::StdRng, SeedableRng};

use crate::{cancellation::CancellationToken, domain::TimeOfDay};

use super::{
    AutoPerfectMatching, AverageSymmetricDistance, ChristofidesInitialRoute, ClusteredInitialRoute,
    ClusteredSolver, ClusteredSolverConfig, DefaultObjectivePolicy, ExactBitDpSolver,
    GreedyInitialRoute, InitialRouteGenerator, MatchingStrategyConfig, MixedNeighborhoodStrategy,
    MstDoubleTreeInitialRoute, ObjectiveEvaluator, RouteSolver, SimulatedAnnealingConfig,
    SimulatedAnnealingSolver, SolverCandidate, SolverCandidateMetadata, SolverDiagnostics,
    SolverError, SolverInput, SolverRunResult, SolverSolution, EXACT_MAX_LOCATIONS,
    TIME_SLOT_MINUTES,
};

const DEFAULT_MAX_CONCURRENCY: usize = 4;
const DEFAULT_STRATEGY_TIMEOUT: Duration = Duration::from_secs(30);
const TIMEOUT_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Configuration shared by all strategies in one orchestration run.
#[derive(Debug, Clone, PartialEq)]
pub struct SolverOrchestratorConfig {
    pub exact_limit: usize,
    pub max_cluster_size: usize,
    pub max_concurrency: usize,
    pub strategy_timeout: Duration,
    pub sa_config: SimulatedAnnealingConfig,
    pub sa_seeds: Vec<u64>,
    pub matching: MatchingStrategyConfig,
}

impl Default for SolverOrchestratorConfig {
    fn default() -> Self {
        Self {
            exact_limit: EXACT_MAX_LOCATIONS,
            max_cluster_size: super::DEFAULT_MAX_EXACT_CLUSTER_SIZE,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            strategy_timeout: DEFAULT_STRATEGY_TIMEOUT,
            sa_config: SimulatedAnnealingConfig::default(),
            sa_seeds: vec![42],
            matching: MatchingStrategyConfig::default(),
        }
    }
}

impl SolverOrchestratorConfig {
    pub fn validate(self) -> Result<Self, SolverError> {
        if self.exact_limit == 0 || self.exact_limit > EXACT_MAX_LOCATIONS {
            return Err(SolverError::InvalidConfiguration(format!(
                "orchestrator exact_limit must be between 1 and {EXACT_MAX_LOCATIONS}"
            )));
        }
        ClusteredSolverConfig::new(self.max_cluster_size)?;
        if self.max_concurrency == 0 {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator max_concurrency must be positive".to_owned(),
            ));
        }
        if self.strategy_timeout.is_zero() {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator strategy_timeout must be positive".to_owned(),
            ));
        }
        if self.sa_seeds.is_empty() {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator sa_seeds must not be empty".to_owned(),
            ));
        }
        if self.sa_seeds.iter().copied().collect::<HashSet<_>>().len() != self.sa_seeds.len() {
            return Err(SolverError::InvalidConfiguration(
                "orchestrator sa_seeds must be unique".to_owned(),
            ));
        }
        self.sa_config.validate()?;
        MatchingStrategyConfig::new(self.matching.strategy, self.matching.bit_dp_threshold)?;
        Ok(self)
    }
}

/// Result produced by a strategy before common feasibility/objective
/// evaluation is applied by the orchestrator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyExecution {
    pub solution: SolverSolution,
    pub metadata: SolverCandidateMetadata,
}

/// Extension point for independently executable route strategies.
pub trait OrchestratedStrategy: Send + Sync {
    fn name(&self) -> &str;

    fn is_applicable(&self, _location_count: usize) -> bool {
        true
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError>;
}

/// Applies one objective evaluator to every candidate and deterministically
/// keeps the first registered strategy when all objective fields tie.
pub struct CandidateSelector<'a, E> {
    evaluator: &'a E,
}

impl<'a, E: ObjectiveEvaluator> CandidateSelector<'a, E> {
    pub fn new(evaluator: &'a E) -> Self {
        Self { evaluator }
    }

    pub fn rank(&self, candidates: &mut [SolverCandidate]) {
        candidates.sort_by(|left, right| self.compare_candidates(left, right));
    }

    pub fn select<'b>(
        &self,
        candidates: &'b [SolverCandidate],
    ) -> Result<&'b SolverCandidate, SolverError> {
        candidates
            .iter()
            .find(|candidate| candidate.feasible && candidate.objective_score.is_some())
            .ok_or(SolverError::NoFeasibleRoute)
    }

    fn compare_candidates(&self, left: &SolverCandidate, right: &SolverCandidate) -> Ordering {
        match (left.feasible, right.feasible) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (true, true) => match (&left.objective_score, &right.objective_score) {
                (Some(left), Some(right)) => self.evaluator.compare(left, right),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            (false, false) => Ordering::Equal,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorSolveResult {
    pub solution: SolverSolution,
    pub selected_strategy: String,
    pub candidates: Vec<SolverCandidate>,
}

pub struct SolverOrchestrator<E = DefaultObjectivePolicy> {
    config: SolverOrchestratorConfig,
    evaluator: E,
    strategies: Vec<Arc<dyn OrchestratedStrategy>>,
}

impl Default for SolverOrchestrator<DefaultObjectivePolicy> {
    fn default() -> Self {
        Self::new(SolverOrchestratorConfig::default())
            .expect("the default orchestrator configuration is valid")
    }
}

impl SolverOrchestrator<DefaultObjectivePolicy> {
    pub fn new(config: SolverOrchestratorConfig) -> Result<Self, SolverError> {
        let config = config.validate()?;
        let strategies = built_in_strategies(&config)?;
        Ok(Self {
            config,
            evaluator: DefaultObjectivePolicy,
            strategies,
        })
    }
}

impl<E: ObjectiveEvaluator> SolverOrchestrator<E> {
    /// Primarily useful for adding application-specific strategies and for
    /// testing failure/timeout isolation.
    pub fn with_strategies(
        config: SolverOrchestratorConfig,
        evaluator: E,
        strategies: Vec<Arc<dyn OrchestratedStrategy>>,
    ) -> Result<Self, SolverError> {
        let config = config.validate()?;
        validate_strategy_names(&strategies)?;
        Ok(Self {
            config,
            evaluator,
            strategies,
        })
    }

    pub fn config(&self) -> &SolverOrchestratorConfig {
        &self.config
    }

    pub fn solve_detailed(
        &self,
        input: SolverInput<'_>,
    ) -> Result<OrchestratorSolveResult, SolverError> {
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
        let applicable: Vec<_> = self
            .strategies
            .iter()
            .enumerate()
            .filter(|(_, strategy)| strategy.is_applicable(location_count))
            .map(|(index, strategy)| (index, Arc::clone(strategy)))
            .collect();
        if applicable.is_empty() {
            return Err(SolverError::NoFeasibleRoute);
        }

        let queue = Mutex::new(VecDeque::from(applicable));
        let worker_count = self
            .config
            .max_concurrency
            .min(self.strategies.len())
            .max(1);
        let (result_tx, result_rx) = mpsc::channel();
        thread::scope(|scope| {
            for _ in 0..worker_count {
                let result_tx = result_tx.clone();
                let queue = &queue;
                let evaluator = &self.evaluator;
                let timeout = self.config.strategy_timeout;
                scope.spawn(move || loop {
                    let task = queue
                        .lock()
                        .expect("strategy queue mutex was poisoned")
                        .pop_front();
                    let Some((index, strategy)) = task else {
                        break;
                    };
                    let candidate = execute_strategy(
                        strategy.as_ref(),
                        evaluator,
                        SolverInput {
                            matrix: input.matrix,
                            problem: input.problem,
                            cancellation: input.cancellation,
                        },
                        timeout,
                    );
                    if result_tx.send((index, candidate)).is_err() {
                        break;
                    }
                });
            }
        });
        drop(result_tx);

        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let mut indexed: Vec<_> = result_rx.into_iter().collect();
        indexed.sort_by_key(|(index, _)| *index);
        let mut candidates: Vec<_> = indexed
            .into_iter()
            .map(|(_, candidate)| candidate)
            .collect();
        let selector = CandidateSelector::new(&self.evaluator);
        selector.rank(&mut candidates);
        let selected = selector.select(&candidates)?;
        let solution = selected.route.clone().ok_or(SolverError::NoFeasibleRoute)?;
        Ok(OrchestratorSolveResult {
            solution,
            selected_strategy: selected.strategy.clone(),
            candidates,
        })
    }
}

fn validate_strategy_names(
    strategies: &[Arc<dyn OrchestratedStrategy>],
) -> Result<(), SolverError> {
    let mut names = HashSet::with_capacity(strategies.len());
    for strategy in strategies {
        if strategy.name().trim().is_empty() || !names.insert(strategy.name()) {
            return Err(SolverError::InvalidConfiguration(
                "orchestrated strategy names must be non-empty and unique".to_owned(),
            ));
        }
    }
    Ok(())
}

impl<E: ObjectiveEvaluator> RouteSolver for SolverOrchestrator<E> {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        self.solve_detailed(input).map(|result| result.solution)
    }

    fn solve_with_diagnostics(
        &self,
        input: SolverInput<'_>,
    ) -> Result<SolverRunResult, SolverError> {
        self.solve_detailed(input).map(|result| SolverRunResult {
            solution: result.solution,
            diagnostics: Some(SolverDiagnostics {
                selected_strategy: result.selected_strategy,
                candidates: result.candidates,
            }),
        })
    }

    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        let metrics = self.evaluator.evaluate(input, solution)?;
        TimeOfDay::from_minutes(metrics.start_time_slot * TIME_SLOT_MINUTES as u16)
            .map_err(|error| SolverError::Failed(error.to_string()))
    }
}

fn execute_strategy<E: ObjectiveEvaluator>(
    strategy: &dyn OrchestratedStrategy,
    evaluator: &E,
    input: SolverInput<'_>,
    timeout: Duration,
) -> SolverCandidate {
    let started = Instant::now();
    let child_cancellation = CancellationToken::new();
    let timeout_reason = AtomicU8::new(0);
    let (done_tx, done_rx) = mpsc::channel();

    let outcome = thread::scope(|scope| {
        let parent_cancellation = input.cancellation;
        let watcher_cancellation = child_cancellation.clone();
        let timeout_reason = &timeout_reason;
        let watcher = scope.spawn(move || {
            let watcher_started = Instant::now();
            loop {
                if parent_cancellation.is_cancelled() {
                    timeout_reason.store(2, AtomicOrdering::Release);
                    watcher_cancellation.cancel();
                    break;
                }
                let elapsed = watcher_started.elapsed();
                if elapsed >= timeout {
                    timeout_reason.store(1, AtomicOrdering::Release);
                    watcher_cancellation.cancel();
                    break;
                }
                let wait = timeout
                    .saturating_sub(elapsed)
                    .min(TIMEOUT_POLL_INTERVAL);
                match done_rx.recv_timeout(wait) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        });
        let result = catch_unwind(AssertUnwindSafe(|| {
            let execution = strategy.execute(SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: &child_cancellation,
            })?;
            let evaluated = evaluator.evaluate(
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: &child_cancellation,
                },
                &execution.solution,
            );
            Ok::<_, SolverError>((execution, evaluated))
        }));
        let _ = done_tx.send(());
        let _ = watcher.join();
        result
    });

    let elapsed = started.elapsed();
    let reason = timeout_reason.load(AtomicOrdering::Acquire);
    if reason == 1 {
        return failed_candidate(
            strategy.name(),
            elapsed,
            true,
            "strategy timed out".to_owned(),
        );
    }
    if reason == 2 {
        return failed_candidate(
            strategy.name(),
            elapsed,
            false,
            SolverError::Cancelled.to_string(),
        );
    }
    match outcome {
        Err(payload) => failed_candidate(
            strategy.name(),
            elapsed,
            false,
            format!("strategy panicked: {}", panic_message(payload)),
        ),
        Ok(Err(error)) => failed_candidate(strategy.name(), elapsed, false, error.to_string()),
        Ok(Ok((execution, Ok(metrics)))) => SolverCandidate {
            strategy: strategy.name().to_owned(),
            route: Some(execution.solution),
            feasible: true,
            objective_score: Some(metrics),
            elapsed,
            metadata: execution.metadata,
        },
        Ok(Ok((execution, Err(error)))) => {
            let mut metadata = execution.metadata;
            metadata.error = Some(error.to_string());
            SolverCandidate {
                strategy: strategy.name().to_owned(),
                route: Some(execution.solution),
                feasible: false,
                objective_score: None,
                elapsed,
                metadata,
            }
        }
    }
}

fn failed_candidate(
    strategy: &str,
    elapsed: Duration,
    timed_out: bool,
    error: String,
) -> SolverCandidate {
    SolverCandidate {
        strategy: strategy.to_owned(),
        route: None,
        feasible: false,
        objective_score: None,
        elapsed,
        metadata: SolverCandidateMetadata {
            timed_out,
            error: Some(error),
            ..SolverCandidateMetadata::default()
        },
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

fn built_in_strategies(
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
        Arc::new(InitialGeneratorStrategy {
            name: "mst_double_tree".to_owned(),
            seed: 0,
            generator: MstDoubleTreeInitialRoute::default(),
        }),
        Arc::new(InitialGeneratorStrategy {
            name: "christofides".to_owned(),
            seed: 0,
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
            seed,
            sa_config,
            GreedyInitialRoute,
        )?));
        strategies.push(Arc::new(AnnealingStrategy::new(
            format!("sa_mst_seed_{seed}"),
            seed,
            sa_config,
            MstDoubleTreeInitialRoute::default(),
        )?));
        strategies.push(Arc::new(AnnealingStrategy::new(
            format!("sa_christofides_seed_{seed}"),
            seed,
            sa_config,
            ChristofidesInitialRoute::new(AverageSymmetricDistance, matching),
        )?));
        strategies.push(Arc::new(AnnealingStrategy::new(
            format!("sa_clustered_seed_{seed}"),
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
        let result = self.solver.solve_detailed(input)?;
        Ok(StrategyExecution {
            solution: result.solution,
            metadata: SolverCandidateMetadata {
                state_count: Some(result.stats.exact_generated_states),
                frontier_state_count: Some(result.stats.exact_frontier_states),
                cluster_count: Some(result.stats.cluster_count),
                improved_moves: Some(result.stats.accepted_local_moves as u64),
                ..SolverCandidateMetadata::default()
            },
        })
    }
}

struct InitialGeneratorStrategy<G> {
    name: String,
    seed: u64,
    generator: G,
}

impl<G: InitialRouteGenerator> OrchestratedStrategy for InitialGeneratorStrategy<G> {
    fn name(&self) -> &str {
        &self.name
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        let mut rng = StdRng::seed_from_u64(self.seed);
        let solution = self.generator.generate(input, &mut rng)?;
        Ok(StrategyExecution {
            solution,
            metadata: SolverCandidateMetadata {
                seed: Some(self.seed),
                ..SolverCandidateMetadata::default()
            },
        })
    }
}

struct AnnealingStrategy<I> {
    name: String,
    seed: u64,
    solver: SimulatedAnnealingSolver<I, MixedNeighborhoodStrategy, DefaultObjectivePolicy>,
}

impl<I> AnnealingStrategy<I> {
    fn new(
        name: String,
        seed: u64,
        config: SimulatedAnnealingConfig,
        initial: I,
    ) -> Result<Self, SolverError> {
        Ok(Self {
            name,
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

impl<I: super::InitialRouteStrategy> OrchestratedStrategy for AnnealingStrategy<I> {
    fn name(&self) -> &str {
        &self.name
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        let result = self.solver.solve_detailed(input)?;
        Ok(StrategyExecution {
            solution: result.solution,
            metadata: SolverCandidateMetadata {
                iteration_count: Some(result.stats.iterations),
                accepted_moves: Some(result.stats.accepted_moves),
                improved_moves: Some(result.stats.improved_moves),
                seed: Some(self.seed),
                ..SolverCandidateMetadata::default()
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering as TestOrdering},
        Arc,
    };

    use super::*;
    use crate::{
        api::OptimizeRouteRequest, domain::OptimizationProblem, matrix::TravelTimeMatrix,
        solver::SolutionMetrics,
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
