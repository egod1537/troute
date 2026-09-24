use std::{
    any::Any,
    collections::{HashSet, VecDeque},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU8, Ordering as AtomicOrdering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    cancellation::CancellationToken,
    domain::TimeOfDay,
    solver::{
        DefaultObjectivePolicy, ObjectiveEvaluator, RouteSolver, SolverCandidate,
        SolverCandidateMetadata, SolverDiagnostics, SolverError, SolverInput, SolverRunResult,
        SolverSolution,
    },
};

use super::{
    diagnostics::{annealing_strategy, built_in_strategies, SaInitializer},
    CandidateSelector, SolverOrchestratorConfig,
};

const TIMEOUT_POLL_INTERVAL: Duration = Duration::from_millis(2);

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

    fn preserves_result_on_cancellation(&self) -> bool {
        false
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorSolveResult {
    pub solution: SolverSolution,
    pub selected_strategy: String,
    pub candidates: Vec<SolverCandidate>,
    pub total_budget_ms: u64,
    pub total_elapsed_ms: u64,
    pub baseline_elapsed_ms: u64,
    pub sa_elapsed_ms: u64,
    pub sa_run_count: u64,
    pub global_best_updates: u64,
    pub termination_reason: String,
}

/// Replaceable ordering policy for SA initializers. The default guarantees the
/// four baseline sources first, then interleaves warm starts and deterministic
/// restarts.
pub trait InitializerSelectionPolicy: Send + Sync {
    fn select(&self, run_index: u64, has_global_best: bool) -> SaInitializer;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RoundRobinInitializerSelectionPolicy;

impl InitializerSelectionPolicy for RoundRobinInitializerSelectionPolicy {
    fn select(&self, run_index: u64, has_global_best: bool) -> SaInitializer {
        const BASELINES: [SaInitializer; 4] = [
            SaInitializer::Greedy,
            SaInitializer::MstDoubleTree,
            SaInitializer::Christofides,
            SaInitializer::Clustered,
        ];
        if run_index < BASELINES.len() as u64 {
            return BASELINES[run_index as usize];
        }
        let restart = (run_index - BASELINES.len() as u64) % 5;
        if restart == 0 && has_global_best {
            SaInitializer::GlobalBest
        } else {
            BASELINES[restart.saturating_sub(1) as usize % BASELINES.len()]
        }
    }
}

pub fn deterministic_sa_seed(base_seed: u64, initializer: SaInitializer, round: u64) -> u64 {
    let tag = match initializer {
        SaInitializer::Greedy => 1_u64,
        SaInitializer::MstDoubleTree => 2,
        SaInitializer::Christofides => 3,
        SaInitializer::Clustered => 4,
        SaInitializer::GlobalBest => 5,
    };
    let mut value = base_seed ^ tag.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ round.rotate_left(17);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

pub struct SolverOrchestrator<E = DefaultObjectivePolicy> {
    config: SolverOrchestratorConfig,
    evaluator: E,
    strategies: Vec<Arc<dyn OrchestratedStrategy>>,
    anytime: bool,
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
            anytime: true,
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
            anytime: false,
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
        if self.anytime {
            return self.solve_anytime(input);
        }
        self.solve_registered(input)
    }

    fn solve_registered(
        &self,
        input: SolverInput<'_>,
    ) -> Result<OrchestratorSolveResult, SolverError> {
        let started = Instant::now();
        let location_count = input.problem.locations().len();
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
            total_budget_ms: duration_ms(self.config.total_budget),
            total_elapsed_ms: duration_ms(started.elapsed()),
            baseline_elapsed_ms: duration_ms(started.elapsed()),
            sa_elapsed_ms: 0,
            sa_run_count: 0,
            global_best_updates: 1,
            termination_reason: "exhausted".to_owned(),
        })
    }

    fn solve_anytime(
        &self,
        input: SolverInput<'_>,
    ) -> Result<OrchestratorSolveResult, SolverError> {
        let started = Instant::now();
        let deadline = started + self.config.total_budget;
        let location_count = input.problem.locations().len();
        let mut candidates = Vec::new();
        let mut best_index = None;
        let mut global_best_updates = 0_u64;

        if let Some(exact) = self.strategies.iter().find(|strategy| {
            strategy.name() == "exact_bit_dp" && strategy.is_applicable(location_count)
        }) {
            if let Some(timeout) = strategy_allowance(
                deadline,
                self.config.deadline_safety_margin,
                self.config.strategy_timeout,
            ) {
                let candidate = execute_strategy(
                    exact.as_ref(),
                    &self.evaluator,
                    SolverInput {
                        matrix: input.matrix,
                        problem: input.problem,
                        cancellation: input.cancellation,
                    },
                    timeout,
                );
                update_global_best(
                    &self.evaluator,
                    &candidates,
                    &candidate,
                    &mut best_index,
                    &mut global_best_updates,
                );
                let exact_optimum = candidate.feasible && candidate.route.is_some();
                candidates.push(candidate);
                if exact_optimum {
                    return build_result(
                        &self.evaluator,
                        self.config.total_budget,
                        started,
                        started.elapsed(),
                        Duration::ZERO,
                        0,
                        global_best_updates,
                        "exact_optimum",
                        candidates,
                    );
                }
            }
        }

        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }

        let baseline_strategies: Vec<_> = self
            .strategies
            .iter()
            .filter(|strategy| {
                strategy.name() != "exact_bit_dp" && strategy.is_applicable(location_count)
            })
            .cloned()
            .collect();
        let allowance = strategy_allowance(
            deadline,
            self.config.deadline_safety_margin,
            self.config.strategy_timeout,
        );
        if allowance.is_some() {
            let queue = Mutex::new(VecDeque::from(
                baseline_strategies
                    .into_iter()
                    .enumerate()
                    .collect::<Vec<_>>(),
            ));
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
                    scope.spawn(move || loop {
                        if input.cancellation.is_cancelled() {
                            break;
                        }
                        let task = queue
                            .lock()
                            .expect("strategy queue mutex was poisoned")
                            .pop_front();
                        let Some((index, strategy)) = task else { break };
                        let Some(timeout) = strategy_allowance(
                            deadline,
                            self.config.deadline_safety_margin,
                            self.config.strategy_timeout,
                        ) else {
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
            let mut baseline_results: Vec<_> = result_rx.into_iter().collect();
            baseline_results.sort_by_key(|(index, _)| *index);
            for (_, candidate) in baseline_results {
                update_global_best(
                    &self.evaluator,
                    &candidates,
                    &candidate,
                    &mut best_index,
                    &mut global_best_updates,
                );
                candidates.push(candidate);
            }
        }
        let baseline_elapsed = started.elapsed();
        let sa_started = Instant::now();
        let policy = RoundRobinInitializerSelectionPolicy;
        let mut sa_run_count = 0_u64;
        let mut exhausted = false;

        while !input.cancellation.is_cancelled() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining <= self.config.deadline_safety_margin {
                break;
            }
            let usable = remaining - self.config.deadline_safety_margin;
            let chunk = self.config.sa_chunk.min(usable);
            if chunk.is_zero() {
                break;
            }
            let initializer = policy.select(sa_run_count, best_index.is_some());
            let warm_start = best_index.and_then(|index| candidates[index].route.clone());
            let round = sa_run_count / 4;
            let base_seed = self.config.sa_seeds[(round as usize) % self.config.sa_seeds.len()];
            let seed = deterministic_sa_seed(base_seed, initializer, round);
            let strategy = annealing_strategy(
                &self.config,
                initializer,
                warm_start,
                seed,
                sa_run_count + 1,
                duration_ms(chunk).saturating_sub(1).max(1),
            )?;
            let mut candidate = execute_strategy(
                strategy.as_ref(),
                &self.evaluator,
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                chunk,
            );
            sa_run_count += 1;
            candidate
                .metadata
                .initial_strategy
                .get_or_insert_with(|| initializer.label().to_owned());
            candidate.metadata.seed.get_or_insert(seed);
            let improved = update_global_best(
                &self.evaluator,
                &candidates,
                &candidate,
                &mut best_index,
                &mut global_best_updates,
            );
            candidate.metadata.improved_global_best = Some(improved);
            let no_iterations = candidate.metadata.iteration_count == Some(0);
            candidates.push(candidate);
            if no_iterations {
                exhausted = true;
                break;
            }
        }

        let termination_reason = if input.cancellation.is_cancelled() {
            "cancellation"
        } else if exhausted {
            "exhausted"
        } else if Instant::now()
            >= deadline
                .checked_sub(self.config.deadline_safety_margin)
                .unwrap_or(deadline)
        {
            "deadline"
        } else {
            "exhausted"
        };
        let sa_elapsed = sa_started.elapsed();
        build_result(
            &self.evaluator,
            self.config.total_budget,
            started,
            baseline_elapsed,
            sa_elapsed,
            sa_run_count,
            global_best_updates,
            termination_reason,
            candidates,
        )
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
                total_budget_ms: result.total_budget_ms,
                total_elapsed_ms: result.total_elapsed_ms,
                baseline_elapsed_ms: result.baseline_elapsed_ms,
                sa_elapsed_ms: result.sa_elapsed_ms,
                sa_run_count: result.sa_run_count,
                global_best_updates: result.global_best_updates,
                termination_reason: result.termination_reason,
            }),
        })
    }

    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        let metrics = self.evaluator.evaluate(input, solution)?;
        crate::solver::selected_start_time(input.problem, &metrics)
    }
}

fn strategy_allowance(
    deadline: Instant,
    safety_margin: Duration,
    strategy_timeout: Duration,
) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (remaining > safety_margin).then(|| strategy_timeout.min(remaining - safety_margin))
}

fn update_global_best<E: ObjectiveEvaluator>(
    evaluator: &E,
    candidates: &[SolverCandidate],
    candidate: &SolverCandidate,
    best_index: &mut Option<usize>,
    updates: &mut u64,
) -> bool {
    if !candidate.feasible || candidate.route.is_none() {
        return false;
    }
    let Some(metrics) = candidate.objective_score.as_ref() else {
        return false;
    };
    let improves = match *best_index {
        Some(index) => candidates[index]
            .objective_score
            .as_ref()
            .is_none_or(|best| evaluator.compare(metrics, best).is_lt()),
        None => true,
    };
    if improves {
        *best_index = Some(candidates.len());
        *updates += 1;
    }
    improves
}

#[allow(clippy::too_many_arguments)]
fn build_result<E: ObjectiveEvaluator>(
    evaluator: &E,
    total_budget: Duration,
    started: Instant,
    baseline_elapsed: Duration,
    sa_elapsed: Duration,
    sa_run_count: u64,
    global_best_updates: u64,
    termination_reason: &str,
    mut candidates: Vec<SolverCandidate>,
) -> Result<OrchestratorSolveResult, SolverError> {
    let selector = CandidateSelector::new(evaluator);
    selector.rank(&mut candidates);
    let selected = selector.select(&candidates)?;
    let solution = selected.route.clone().ok_or(SolverError::NoFeasibleRoute)?;
    let selected_strategy = selected.strategy.clone();
    Ok(OrchestratorSolveResult {
        solution,
        selected_strategy,
        candidates,
        total_budget_ms: duration_ms(total_budget),
        total_elapsed_ms: duration_ms(started.elapsed()),
        baseline_elapsed_ms: duration_ms(baseline_elapsed),
        sa_elapsed_ms: duration_ms(sa_elapsed),
        sa_run_count,
        global_best_updates,
        termination_reason: termination_reason.to_owned(),
    })
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
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
                let wait = timeout.saturating_sub(elapsed).min(TIMEOUT_POLL_INTERVAL);
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
            let validation_cancellation = CancellationToken::new();
            let evaluation_cancellation = if strategy.preserves_result_on_cancellation()
                && child_cancellation.is_cancelled()
            {
                &validation_cancellation
            } else {
                &child_cancellation
            };
            let evaluated = evaluator.evaluate(
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: evaluation_cancellation,
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
    if reason == 1 && !strategy.preserves_result_on_cancellation() {
        return failed_candidate(
            strategy.name(),
            elapsed,
            true,
            "strategy timed out".to_owned(),
        );
    }
    if reason == 2 && !strategy.preserves_result_on_cancellation() {
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
        Ok(Ok((execution, Ok(metrics)))) => {
            let mut metadata = execution.metadata;
            metadata.timed_out = reason == 1;
            SolverCandidate {
                strategy: strategy.name().to_owned(),
                route: Some(execution.solution),
                feasible: true,
                objective_score: Some(metrics),
                elapsed,
                metadata,
            }
        }
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
