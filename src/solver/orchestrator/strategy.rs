use std::{
    any::Any,
    collections::VecDeque,
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
    solver::{
        ObjectiveEvaluator, ResourceManager, ResourceRequest, SolverCandidate,
        SolverCandidateMetadata, SolverError, SolverId, SolverInput, SolverOutcomeKind,
        SolverProgressEvent, SolverProgressObserver, SolverProofStatus, SolverSolution,
    },
};

use super::SolverOrchestratorConfig;

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

    fn is_applicable_to(&self, problem: &crate::domain::OptimizationProblem) -> bool {
        self.is_applicable(problem.locations().len())
    }

    fn preserves_result_on_cancellation(&self) -> bool {
        false
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError>;

    fn execute_with_previews(
        &self,
        input: SolverInput<'_>,
        _enabled: bool,
    ) -> Result<StrategyExecution, SolverError> {
        self.execute(input)
    }
}

pub(super) fn report_progress(
    observer: &dyn SolverProgressObserver,
    event: impl FnOnce() -> SolverProgressEvent,
) {
    if observer.is_enabled() {
        observer.on_progress(event());
    }
}

#[derive(Clone, Copy)]
pub(super) enum StrategyTimeoutPolicy {
    Fixed(Duration),
    RemainingBudget {
        deadline: Instant,
        safety_margin: Duration,
        cap: Duration,
    },
}

impl StrategyTimeoutPolicy {
    fn allowance(self) -> Option<Duration> {
        match self {
            Self::Fixed(timeout) => Some(timeout),
            Self::RemainingBudget {
                deadline,
                safety_margin,
                cap,
            } => strategy_allowance(deadline, safety_margin, cap),
        }
    }

    fn stops_when_cancelled(self) -> bool {
        matches!(self, Self::RemainingBudget { .. })
    }
}

/// Executes a bounded batch while preserving registration order in the
/// returned candidates.
pub(super) struct StrategyBatchExecutor<'a, E> {
    pub input: SolverInput<'a>,
    pub observer: &'a dyn SolverProgressObserver,
    pub evaluator: &'a E,
    pub resources: &'a ResourceManager,
    pub config: &'a SolverOrchestratorConfig,
    pub started: Instant,
    pub timeout_policy: StrategyTimeoutPolicy,
}

impl<E: ObjectiveEvaluator> StrategyBatchExecutor<'_, E> {
    pub fn execute(
        &self,
        tasks: Vec<(usize, Arc<dyn OrchestratedStrategy>)>,
        worker_count: usize,
    ) -> Vec<(usize, SolverCandidate)> {
        let queue = Mutex::new(VecDeque::from(tasks));
        let (result_tx, result_rx) = mpsc::channel();
        thread::scope(|scope| {
            for _ in 0..worker_count.max(1) {
                let result_tx = result_tx.clone();
                let queue = &queue;
                scope.spawn(move || loop {
                    if self.timeout_policy.stops_when_cancelled()
                        && self.input.cancellation.is_cancelled()
                    {
                        break;
                    }
                    let task = queue
                        .lock()
                        .expect("strategy queue mutex was poisoned")
                        .pop_front();
                    let Some((index, strategy)) = task else {
                        break;
                    };
                    let Some(timeout) = self.timeout_policy.allowance() else {
                        break;
                    };
                    report_progress(self.observer, || {
                        SolverProgressEvent::StrategyStarted(strategy.name().to_owned())
                    });
                    let candidate = execute_strategy(
                        strategy.as_ref(),
                        self.evaluator,
                        self.observer,
                        self.input,
                        timeout,
                        self.started,
                        self.resources,
                        resource_request(self.config, strategy.name()),
                    );
                    report_progress(self.observer, || {
                        SolverProgressEvent::StrategyCompleted(strategy.name().to_owned())
                    });
                    self.observer.on_candidate(&candidate);
                    if result_tx.send((index, candidate)).is_err() {
                        break;
                    }
                });
            }
        });
        drop(result_tx);
        let mut results: Vec<_> = result_rx.into_iter().collect();
        results.sort_by_key(|(index, _)| *index);
        results
    }
}

pub(super) fn strategy_allowance(
    deadline: Instant,
    safety_margin: Duration,
    strategy_timeout: Duration,
) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    (remaining > safety_margin).then(|| strategy_timeout.min(remaining - safety_margin))
}

pub(super) fn solver_id_for_strategy(strategy: &str) -> SolverId {
    let stable = if strategy.starts_with("sa_") {
        "sa"
    } else {
        strategy
    };
    SolverId::new(stable).unwrap_or_else(|_| SolverId::new("custom").expect("static ID is valid"))
}

pub(super) fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute_strategy<E: ObjectiveEvaluator>(
    strategy: &dyn OrchestratedStrategy,
    evaluator: &E,
    observer: &dyn SolverProgressObserver,
    input: SolverInput<'_>,
    timeout: Duration,
    job_started: Instant,
    resources: &ResourceManager,
    resource_request: ResourceRequest,
) -> SolverCandidate {
    let queued_at = Instant::now();
    let job_queued_at_ms = duration_ms(job_started.elapsed());
    let resource_deadline = queued_at + timeout;
    let lease =
        match resources.acquire_until(resource_request, resource_deadline, input.cancellation) {
            Ok(lease) => lease,
            Err(crate::solver::ResourceError::Cancelled) => {
                let mut candidate = failed_candidate(
                    strategy.name(),
                    queued_at.elapsed(),
                    false,
                    SolverError::Cancelled.to_string(),
                );
                candidate.metadata.job_queued_at_ms = Some(job_queued_at_ms);
                candidate.metadata.job_finished_at_ms = Some(duration_ms(job_started.elapsed()));
                return candidate;
            }
            Err(error) => {
                let mut candidate = failed_candidate(
                    strategy.name(),
                    queued_at.elapsed(),
                    matches!(error, crate::solver::ResourceError::DeadlineExceeded),
                    error.to_string(),
                );
                candidate.metadata.job_queued_at_ms = Some(job_queued_at_ms);
                candidate.metadata.job_finished_at_ms = Some(duration_ms(job_started.elapsed()));
                return candidate;
            }
        };
    let queued_ms = duration_ms(queued_at.elapsed());
    let timeout = resource_deadline.saturating_duration_since(Instant::now());
    if timeout.is_zero() {
        let mut candidate = failed_candidate(
            strategy.name(),
            queued_at.elapsed(),
            true,
            "strategy deadline elapsed while waiting for resources".to_owned(),
        );
        candidate.metadata.job_queued_at_ms = Some(job_queued_at_ms);
        candidate.metadata.job_finished_at_ms = Some(duration_ms(job_started.elapsed()));
        return candidate;
    }
    let started = Instant::now();
    let job_started_at_ms = duration_ms(job_started.elapsed());
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
            let execution = strategy.execute_with_previews(
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: &child_cancellation,
                },
                observer.is_enabled(),
            )?;
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
    let mut candidate = if reason == 1 && !strategy.preserves_result_on_cancellation() {
        failed_candidate(
            strategy.name(),
            elapsed,
            true,
            "strategy timed out".to_owned(),
        )
    } else if reason == 2 && !strategy.preserves_result_on_cancellation() {
        failed_candidate(
            strategy.name(),
            elapsed,
            false,
            SolverError::Cancelled.to_string(),
        )
    } else {
        match outcome {
            Err(payload) => failed_candidate(
                strategy.name(),
                elapsed,
                false,
                format!("strategy panicked: {}", panic_message(payload)),
            ),
            Ok(Err(error)) => failed_candidate_from_error(strategy.name(), elapsed, error),
            Ok(Ok((execution, Ok(metrics)))) => {
                let mut metadata = execution.metadata;
                let samples = metadata.preview_samples.take().unwrap_or_default();
                if !input.cancellation.is_cancelled() {
                    for (sample, _) in samples {
                        // Validate accepted states again through the authoritative evaluator.
                        if let Ok(metrics) = evaluator.evaluate(input, &sample) {
                            observer.on_search_sample(&sample, &metrics);
                        }
                    }
                }
                metadata.timed_out |= reason == 1;
                metadata.outcome = Some(if metadata.timed_out {
                    SolverOutcomeKind::Timeout
                } else {
                    SolverOutcomeKind::Completed
                });
                metadata.proof_status.get_or_insert_with(|| {
                    if strategy.name() == "exact_bit_dp" && reason == 0 {
                        SolverProofStatus::Optimal
                    } else {
                        SolverProofStatus::Feasible
                    }
                });
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
                metadata.preview_samples = None;
                metadata.error = Some(error.to_string());
                metadata.outcome = Some(SolverOutcomeKind::Failed);
                metadata.proof_status = Some(SolverProofStatus::Unknown);
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
    };
    candidate.metadata.solver_id = Some(solver_id_for_strategy(strategy.name()).to_string());
    candidate.metadata.job_queued_at_ms = Some(job_queued_at_ms);
    candidate.metadata.job_started_at_ms = Some(job_started_at_ms);
    candidate.metadata.job_finished_at_ms = Some(duration_ms(job_started.elapsed()));
    candidate.metadata.queued_ms = Some(queued_ms);
    candidate.metadata.thread_count = Some(resource_request.native_threads);
    candidate.metadata.memory_hint_bytes = resource_request.memory_hint_bytes;
    candidate.metadata.reserved_slot_ms = Some(lease.reserved_slot_ms());
    drop(lease);
    candidate
}

pub(super) fn resource_request(
    config: &SolverOrchestratorConfig,
    strategy: &str,
) -> ResourceRequest {
    let solver_id = solver_id_for_strategy(strategy);
    let native_threads = if solver_id.as_str() == "mip_scip" {
        config.mip_scip_threads
    } else {
        1
    };
    ResourceRequest {
        logical_cpu_slots: native_threads,
        native_threads,
        external_processes: usize::from(solver_id.as_str() == "lkh3"),
        memory_hint_bytes: (solver_id.as_str() == "mip_scip")
            .then(|| (config.mip_scip_memory_mb as u64).saturating_mul(1024 * 1024)),
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
            outcome: Some(if timed_out {
                SolverOutcomeKind::Timeout
            } else if error == SolverError::Cancelled.to_string() {
                SolverOutcomeKind::Cancelled
            } else if strategy == "exact_bit_dp"
                && error == SolverError::NoFeasibleRoute.to_string()
            {
                SolverOutcomeKind::Infeasible
            } else {
                SolverOutcomeKind::Failed
            }),
            proof_status: (strategy == "exact_bit_dp"
                && error == SolverError::NoFeasibleRoute.to_string())
            .then_some(SolverProofStatus::Infeasible),
            timed_out,
            error: Some(error),
            ..SolverCandidateMetadata::default()
        },
    }
}

fn failed_candidate_from_error(
    strategy: &str,
    elapsed: Duration,
    error: SolverError,
) -> SolverCandidate {
    let (outcome, proof_status, timed_out) = match &error {
        SolverError::Cancelled => (
            SolverOutcomeKind::Cancelled,
            Some(SolverProofStatus::Unknown),
            false,
        ),
        SolverError::ProvenInfeasible => (
            SolverOutcomeKind::Infeasible,
            Some(SolverProofStatus::Infeasible),
            false,
        ),
        SolverError::Unsupported(_) => (
            SolverOutcomeKind::Unsupported,
            Some(SolverProofStatus::Unknown),
            false,
        ),
        SolverError::TimedOut => (
            SolverOutcomeKind::Timeout,
            Some(SolverProofStatus::Unknown),
            true,
        ),
        _ => (
            SolverOutcomeKind::Failed,
            Some(SolverProofStatus::Unknown),
            false,
        ),
    };
    SolverCandidate {
        strategy: strategy.to_owned(),
        route: None,
        feasible: false,
        objective_score: None,
        elapsed,
        metadata: SolverCandidateMetadata {
            outcome: Some(outcome),
            proof_status,
            timed_out,
            error: Some(error.to_string()),
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
