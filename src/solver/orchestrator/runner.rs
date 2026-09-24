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
        SolverSolution, TIME_SLOT_MINUTES,
    },
};

use super::{diagnostics::built_in_strategies, CandidateSelector, SolverOrchestratorConfig};

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

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError>;
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
