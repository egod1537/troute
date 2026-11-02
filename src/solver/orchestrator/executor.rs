use std::{
    any::Any,
    collections::VecDeque,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU8, Ordering as AtomicOrdering},
        mpsc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    cancellation::CancellationToken,
    solver::{
        ExecutionContext, ObjectiveEvaluator, ResourceError, ResourceManager, SolverCandidate,
        SolverCandidateMetadata, SolverError, SolverInput, SolverOutcomeKind, SolverProgressEvent,
        SolverProgressObserver, SolverProofStatus, TaskResult,
    },
};

use super::{
    scheduler::ScheduledTask,
    strategy::{duration_ms, report_progress},
};

const TIMEOUT_POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Common execution layer for every solver task. Scheduling decisions stay in
/// [`super::TaskScheduler`]; this type only handles runtime mechanics and
/// shared objective validation.
pub struct TaskExecutor<'a, E> {
    evaluator: &'a E,
    resources: &'a ResourceManager,
    observer: &'a dyn SolverProgressObserver,
    job_started: Instant,
    execution_deadline: Option<Instant>,
}

impl<'a, E: ObjectiveEvaluator> TaskExecutor<'a, E> {
    pub fn new(
        evaluator: &'a E,
        resources: &'a ResourceManager,
        observer: &'a dyn SolverProgressObserver,
        job_started: Instant,
        execution_deadline: Option<Instant>,
    ) -> Self {
        Self {
            evaluator,
            resources,
            observer,
            job_started,
            execution_deadline,
        }
    }

    pub fn execute(&self, input: SolverInput<'_>, scheduled: ScheduledTask) -> TaskResult {
        report_progress(self.observer, || {
            SolverProgressEvent::StrategyStarted(scheduled.strategy_name().to_owned())
        });
        let result = self.execute_inner(input, &scheduled);
        report_progress(self.observer, || {
            SolverProgressEvent::StrategyCompleted(scheduled.strategy_name().to_owned())
        });
        result
    }

    /// Executes a bounded batch and returns results in task order, independent
    /// of physical completion order.
    pub fn execute_batch(
        &self,
        input: SolverInput<'_>,
        tasks: Vec<ScheduledTask>,
        worker_count: usize,
    ) -> Vec<TaskResult> {
        let queue = Mutex::new(VecDeque::from(tasks));
        let (result_tx, result_rx) = mpsc::channel();
        thread::scope(|scope| {
            for _ in 0..worker_count.max(1) {
                let result_tx = result_tx.clone();
                let queue = &queue;
                scope.spawn(move || loop {
                    if input.cancellation.is_cancelled() {
                        break;
                    }
                    let scheduled = queue
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .pop_front();
                    let Some(scheduled) = scheduled else {
                        break;
                    };
                    let result = self.execute(input, scheduled);
                    if result_tx.send(result).is_err() {
                        break;
                    }
                });
            }
        });
        drop(result_tx);
        let mut results: Vec<_> = result_rx.into_iter().collect();
        results.sort_by_key(|result| result.task_id.get());
        results
    }

    fn execute_inner(&self, input: SolverInput<'_>, scheduled: &ScheduledTask) -> TaskResult {
        let task = &scheduled.task;
        let queued_at = Instant::now();
        let job_queued_at_ms = duration_ms(self.job_started.elapsed());
        let task_deadline = queued_at + task.timeout;
        let resource_deadline = self
            .execution_deadline
            .map_or(task_deadline, |deadline| deadline.min(task_deadline));
        let lease = match self.resources.acquire_until(
            task.resources,
            resource_deadline,
            input.cancellation,
        ) {
            Ok(lease) => lease,
            Err(ResourceError::Cancelled) => {
                return self.failed_result(
                    scheduled,
                    queued_at,
                    job_queued_at_ms,
                    false,
                    SolverError::Cancelled.to_string(),
                );
            }
            Err(error) => {
                return self.failed_result(
                    scheduled,
                    queued_at,
                    job_queued_at_ms,
                    matches!(error, ResourceError::DeadlineExceeded),
                    error.to_string(),
                );
            }
        };
        let queued_ms = duration_ms(queued_at.elapsed());
        let remaining = resource_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            drop(lease);
            return self.failed_result(
                scheduled,
                queued_at,
                job_queued_at_ms,
                true,
                "task deadline elapsed while waiting for resources".to_owned(),
            );
        }

        let started = Instant::now();
        let job_started_at_ms = duration_ms(self.job_started.elapsed());
        let child_cancellation = CancellationToken::new();
        let global_deadline = self.execution_deadline.unwrap_or(resource_deadline);
        let context = ExecutionContext::for_task(
            "solver-orchestrator",
            task,
            started,
            global_deadline,
            resource_deadline,
            child_cancellation,
            lease,
        )
        .expect("task executor constructs ordered deadlines and a non-empty job ID");
        let timeout_reason = AtomicU8::new(0);
        let (done_tx, done_rx) = mpsc::channel();

        let outcome = thread::scope(|scope| {
            let parent_cancellation = input.cancellation;
            let watcher_cancellation = context.cancellation().clone();
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
                    if elapsed >= remaining {
                        timeout_reason.store(1, AtomicOrdering::Release);
                        watcher_cancellation.cancel();
                        break;
                    }
                    let wait = remaining.saturating_sub(elapsed).min(TIMEOUT_POLL_INTERVAL);
                    match done_rx.recv_timeout(wait) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            });
            let result = catch_unwind(AssertUnwindSafe(|| {
                let execution = scheduled.strategy().execute(SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: context.cancellation(),
                })?;
                let validation_cancellation = CancellationToken::new();
                let evaluation_cancellation =
                    if scheduled.strategy().preserves_result_on_cancellation()
                        && context.cancellation().is_cancelled()
                    {
                        &validation_cancellation
                    } else {
                        context.cancellation()
                    };
                let evaluated = self.evaluator.evaluate(
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
        let mut candidate =
            if reason == 1 && !scheduled.strategy().preserves_result_on_cancellation() {
                failed_candidate(
                    scheduled.strategy_name(),
                    elapsed,
                    true,
                    "task timed out".to_owned(),
                )
            } else if reason == 2 && !scheduled.strategy().preserves_result_on_cancellation() {
                failed_candidate(
                    scheduled.strategy_name(),
                    elapsed,
                    false,
                    SolverError::Cancelled.to_string(),
                )
            } else {
                match outcome {
                    Err(payload) => failed_candidate(
                        scheduled.strategy_name(),
                        elapsed,
                        false,
                        format!("solver task panicked: {}", panic_message(payload)),
                    ),
                    Ok(Err(error)) => {
                        failed_candidate_from_error(scheduled.strategy_name(), elapsed, error)
                    }
                    Ok(Ok((execution, Ok(metrics)))) => {
                        let mut metadata = execution.metadata;
                        metadata.timed_out |= reason == 1;
                        metadata.outcome = Some(if metadata.timed_out {
                            SolverOutcomeKind::Timeout
                        } else {
                            SolverOutcomeKind::Completed
                        });
                        metadata.proof_status.get_or_insert_with(|| {
                            if task.solver_id.as_str() == "exact_bit_dp" && reason == 0 {
                                SolverProofStatus::Optimal
                            } else {
                                SolverProofStatus::Feasible
                            }
                        });
                        SolverCandidate {
                            strategy: scheduled.strategy_name().to_owned(),
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
                        metadata.outcome = Some(SolverOutcomeKind::Failed);
                        metadata.proof_status = Some(SolverProofStatus::Unknown);
                        SolverCandidate {
                            strategy: scheduled.strategy_name().to_owned(),
                            route: Some(execution.solution),
                            feasible: false,
                            objective_score: None,
                            elapsed,
                            metadata,
                        }
                    }
                }
            };
        candidate.metadata.solver_id = Some(task.solver_id.to_string());
        candidate.metadata.run_id = Some(task.task_id.get());
        candidate.metadata.job_queued_at_ms = Some(job_queued_at_ms);
        candidate.metadata.job_started_at_ms = Some(job_started_at_ms);
        candidate.metadata.job_finished_at_ms = Some(duration_ms(self.job_started.elapsed()));
        candidate.metadata.queued_ms = Some(queued_ms);
        candidate.metadata.thread_count = Some(task.resources.native_threads);
        candidate.metadata.memory_hint_bytes = task.resources.memory_hint_bytes;
        candidate.metadata.seed = candidate.metadata.seed.or(task.seed);
        candidate.metadata.reserved_slot_ms = Some(context.resource_lease().reserved_slot_ms());
        drop(context);
        TaskResult::from_candidate(task, candidate)
    }

    fn failed_result(
        &self,
        scheduled: &ScheduledTask,
        queued_at: Instant,
        job_queued_at_ms: u64,
        timed_out: bool,
        error: String,
    ) -> TaskResult {
        let task = &scheduled.task;
        let mut candidate = failed_candidate(
            scheduled.strategy_name(),
            queued_at.elapsed(),
            timed_out,
            error,
        );
        candidate.metadata.solver_id = Some(task.solver_id.to_string());
        candidate.metadata.run_id = Some(task.task_id.get());
        candidate.metadata.job_queued_at_ms = Some(job_queued_at_ms);
        candidate.metadata.job_finished_at_ms = Some(duration_ms(self.job_started.elapsed()));
        candidate.metadata.thread_count = Some(task.resources.native_threads);
        candidate.metadata.memory_hint_bytes = task.resources.memory_hint_bytes;
        candidate.metadata.seed = task.seed;
        TaskResult::from_candidate(task, candidate)
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
