use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit, Semaphore};

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse},
    cancellation::CancellationToken,
    events::{OptimizationEventReporter, ProgressStage},
    failure::{
        rank_failure_suggestions, service_failure_detail, service_suggestions, FailureDetail,
        FailureSuggestion, DEFAULT_MAX_FAILURE_SUGGESTIONS,
    },
    job_pacing::DebugJobPacer,
    logging::logger,
    routing::RoutingProvider,
    schedule::ScheduleError,
    service::{OptimizationServiceError, RouteOptimizationService},
    solver::{PortfolioCapabilities, RouteSolver, SolverError, TelemetryEvent},
    storage::{
        JobState, JobStatus, JobStore, JobStoreError, StoredJob, StoredJobError,
        TerminalWriteOutcome,
    },
};

mod cancellation;
use cancellation::JobCancellationRegistry;

/// Executes the optimization pipeline independently of its HTTP request.
pub trait JobExecutor: Send + Sync {
    fn validate_route(
        &self,
        _request: crate::validation::ValidateRouteRequest,
        _cancellation: &CancellationToken,
    ) -> Result<crate::validation::ValidateRouteResponse, OptimizationServiceError> {
        Err(OptimizationServiceError::Routing(
            crate::routing::RoutingError::Provider(
                "route validation is unavailable for this executor".to_owned(),
            ),
        ))
    }

    fn apply_request_defaults(&self, _request: &mut OptimizeRouteRequest) {}

    fn execute(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError>;

    fn build_travel_time_matrix(
        &self,
        _request: OptimizeRouteRequest,
    ) -> Result<crate::matrix::TravelTimeMatrix, OptimizationServiceError> {
        Err(OptimizationServiceError::Routing(
            crate::routing::RoutingError::Provider(
                "matrix preview is unavailable for this executor".to_owned(),
            ),
        ))
    }

    fn provider_policy_diagnostics(
        &self,
    ) -> Option<crate::routing::RouteProviderSelectionDiagnostics> {
        None
    }

    fn solver_capabilities(&self) -> Option<PortfolioCapabilities> {
        None
    }

    fn failure_suggestions(
        &self,
        request: &OptimizeRouteRequest,
        error: &OptimizationServiceError,
        _cancellation: &CancellationToken,
    ) -> Vec<crate::failure::FailureSuggestion> {
        rank_failure_suggestions(
            service_suggestions(error, Some(request)),
            DEFAULT_MAX_FAILURE_SUGGESTIONS,
        )
    }
}

impl<P, S> JobExecutor for RouteOptimizationService<P, S>
where
    P: RoutingProvider + Send + Sync,
    S: RouteSolver + Send + Sync,
{
    fn validate_route(
        &self,
        request: crate::validation::ValidateRouteRequest,
        cancellation: &CancellationToken,
    ) -> Result<crate::validation::ValidateRouteResponse, OptimizationServiceError> {
        RouteOptimizationService::validate_route(self, request, cancellation)
    }

    fn apply_request_defaults(&self, request: &mut OptimizeRouteRequest) {
        RouteOptimizationService::apply_request_defaults(self, request);
    }

    fn execute(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        self.optimize_with_cancellation(request, reporter, cancellation)
    }

    fn build_travel_time_matrix(
        &self,
        request: OptimizeRouteRequest,
    ) -> Result<crate::matrix::TravelTimeMatrix, OptimizationServiceError> {
        RouteOptimizationService::build_travel_time_matrix(self, request)
    }

    fn provider_policy_diagnostics(
        &self,
    ) -> Option<crate::routing::RouteProviderSelectionDiagnostics> {
        RouteOptimizationService::provider_policy_diagnostics(self)
    }

    fn solver_capabilities(&self) -> Option<PortfolioCapabilities> {
        RouteOptimizationService::solver_capabilities(self)
    }

    fn failure_suggestions(
        &self,
        request: &OptimizeRouteRequest,
        error: &OptimizationServiceError,
        cancellation: &CancellationToken,
    ) -> Vec<crate::failure::FailureSuggestion> {
        RouteOptimizationService::failure_suggestions(self, request, error, cancellation)
    }
}

/// The persisted Job fields exposed to SSE consumers.
///
/// This deliberately excludes request and persistence lifecycle metadata. All
/// fields are serialized for every event so consumers can replace their
/// current view without merging event-specific shapes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct JobClientState {
    pub job_id: String,
    pub status: JobStatus,
    pub stage: Option<ProgressStage>,
    pub progress: u8,
    pub last_message: Option<String>,
    pub result: Option<OptimizeRouteResponse>,
    pub error: Option<JobClientError>,
}

/// Stable, machine-readable optimization failure sent over SSE.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct JobClientError {
    pub code: String,
    pub message: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_detail: Option<FailureDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<crate::failure::FailureSuggestion>>,
}

impl From<StoredJobError> for JobClientError {
    fn from(error: StoredJobError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            detail: error.detail,
            failure_detail: error.failure_detail,
            suggestions: error.suggestions,
        }
    }
}

/// Canonical payload serialized into every Job SSE `data` field.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct JobEventEnvelope {
    pub sequence: u64,
    pub updated_at: i64,
    pub state: JobClientState,
}

impl JobEventEnvelope {
    /// Builds a wire DTO from a persisted Job without serializing the storage
    /// model itself. `updated_at` is copied from persistence, never generated
    /// at transmission time.
    pub fn from_stored_job(sequence: u64, job: StoredJob) -> Self {
        Self {
            updated_at: job.state.updated_at,
            sequence,
            state: JobClientState {
                job_id: job.state.job_id,
                status: job.state.status,
                stage: job.state.stage,
                progress: job.state.progress,
                last_message: job.state.last_message,
                result: job.result,
                error: job.error.map(JobClientError::from),
            },
        }
    }

    /// Checks the event-name and payload invariants of the public wire
    /// contract. Snapshot accepts every valid persisted status.
    pub fn is_valid_for_event(&self, event: &str) -> bool {
        let payload_is_valid = match self.state.status {
            JobStatus::Pending | JobStatus::Running => {
                self.state.result.is_none() && self.state.error.is_none()
            }
            JobStatus::Completed => self.state.result.is_some() && self.state.error.is_none(),
            JobStatus::Failed => self.state.result.is_none() && self.state.error.is_some(),
            JobStatus::Cancelled => self.state.result.is_none() && self.state.error.is_none(),
        };
        payload_is_valid
            && match event {
                "snapshot" => true,
                "progress" => {
                    matches!(self.state.status, JobStatus::Pending | JobStatus::Running)
                        && self.state.stage.is_some()
                }
                "completed" => self.state.status == JobStatus::Completed,
                "failed" => self.state.status == JobStatus::Failed,
                "cancelled" => self.state.status == JobStatus::Cancelled,
                _ => false,
            }
    }
}

/// Owns submission and background execution of persisted optimization jobs.
#[derive(Clone)]
pub struct JobRunner {
    executor: Arc<dyn JobExecutor>,
    store: Arc<dyn JobStore>,
    cancellations: JobCancellationRegistry,
    permits: Option<Arc<Semaphore>>,
    admission_permits: Option<Arc<Semaphore>>,
    max_outstanding_jobs: Option<usize>,
}

pub(crate) struct JobSubmission {
    pub state: JobState,
    pub completion: oneshot::Receiver<Result<JobCompletion, JobStoreError>>,
}

pub(crate) struct JobCompletion {
    pub result: Result<OptimizeRouteResponse, OptimizationServiceError>,
    pub persisted_error: Option<StoredJobError>,
}

impl JobCompletion {
    fn cancelled() -> Self {
        Self {
            result: Err(OptimizationServiceError::Cancelled),
            persisted_error: None,
        }
    }
}

impl JobRunner {
    pub fn new(
        executor: Arc<dyn JobExecutor>,
        store: Arc<dyn JobStore>,
        max_concurrent_jobs: Option<usize>,
    ) -> Self {
        Self::new_with_limits(executor, store, max_concurrent_jobs, None)
    }

    pub fn new_with_limits(
        executor: Arc<dyn JobExecutor>,
        store: Arc<dyn JobStore>,
        max_concurrent_jobs: Option<usize>,
        max_outstanding_jobs: Option<usize>,
    ) -> Self {
        Self {
            executor,
            store,
            cancellations: JobCancellationRegistry::default(),
            permits: max_concurrent_jobs
                .filter(|limit| *limit > 0)
                .map(|limit| Arc::new(Semaphore::new(limit))),
            admission_permits: max_outstanding_jobs
                .filter(|limit| *limit > 0)
                .map(|limit| Arc::new(Semaphore::new(limit))),
            max_outstanding_jobs: max_outstanding_jobs.filter(|limit| *limit > 0),
        }
    }

    pub(crate) fn activity(&self, job_id: &str) -> Option<JobActivity> {
        self.cancellations.get(job_id)
    }

    pub(crate) fn subscribe(&self, job_id: &str) -> Option<JobEventSubscription> {
        self.cancellations
            .get(job_id)
            .map(|activity| activity.subscribe())
    }

    /// Persists a pending job and schedules its execution without waiting for it.
    pub fn submit(&self, request: OptimizeRouteRequest) -> Result<JobState, JobStoreError> {
        Ok(self.submit_with_completion(request)?.state)
    }

    pub(crate) fn submit_with_completion(
        &self,
        request: OptimizeRouteRequest,
    ) -> Result<JobSubmission, JobStoreError> {
        let admission_permit = self.try_admit()?;
        // Creation is the authoritative duplicate check and happens before any
        // task can observe or execute the job.
        let state = self.store.create_job(&request)?;
        let token = CancellationToken::new();
        let pacer = DebugJobPacer::new(request.debug.as_ref());
        if let Some(minimum_duration_ms) = pacer.minimum_duration_ms() {
            logger().debug_job_pacing_enabled(&request.job_id, minimum_duration_ms);
        }
        self.cancellations
            .insert(&request.job_id, JobActivity::new(token.clone()));

        let (completion_tx, completion) = oneshot::channel();
        let runner = self.clone();
        tokio::spawn(async move {
            let _admission_permit = admission_permit;
            let outcome = runner.run(request, token, pacer).await;
            let _ = completion_tx.send(outcome);
        });

        Ok(JobSubmission { state, completion })
    }

    fn try_admit(&self) -> Result<Option<OwnedSemaphorePermit>, JobStoreError> {
        let Some(permits) = &self.admission_permits else {
            return Ok(None);
        };
        permits
            .clone()
            .try_acquire_owned()
            .map(Some)
            .map_err(|_| JobStoreError::CapacityExceeded {
                maximum: self.max_outstanding_jobs.unwrap_or(0),
            })
    }

    async fn run(
        &self,
        request: OptimizeRouteRequest,
        cancellation: CancellationToken,
        pacer: DebugJobPacer,
    ) -> Result<JobCompletion, JobStoreError> {
        let context = JobExecutionContext::new(request, cancellation, pacer);
        let job_id = context.job_id.clone();
        let outcome = self.run_inner(context).await;
        self.cancellations.remove(&job_id);
        if let Err(error) = &outcome {
            logger().job_operation_failed("job execution persistence", &job_id, error);
        }
        outcome
    }

    async fn run_inner(
        &self,
        context: JobExecutionContext,
    ) -> Result<JobCompletion, JobStoreError> {
        let JobExecutionContext {
            request,
            cancellation,
            pacer,
            job_id,
        } = context;
        let _permit = match &self.permits {
            Some(permits) => {
                tokio::select! {
                    biased;
                    _ = cancellation.cancelled() => return Ok(JobCompletion::cancelled()),
                    permit = permits.acquire() => {
                        Some(permit.expect("job semaphore is not closed"))
                    }
                }
            }
            None => None,
        };
        // A queued job may have been cancelled while waiting for a permit.
        if cancellation.is_cancelled() {
            return Ok(JobCompletion::cancelled());
        }
        match self.store.mark_running(&job_id) {
            Ok(()) => {}
            Err(JobStoreError::InvalidTransition {
                status: JobStatus::Cancelled,
                ..
            }) => return Ok(JobCompletion::cancelled()),
            Err(error) => return Err(error),
        }

        let (execution, update_rx) = self.spawn_worker(request, cancellation.clone());
        let full_result = self
            .consume_updates(&job_id, &pacer, &cancellation, update_rx)
            .await;
        let execution = execution.await;

        let execution_was_cancelled = matches!(
            &execution,
            Ok((Err(OptimizationServiceError::Cancelled), _, _))
        );
        if execution_was_cancelled || !pacer.wait_until_minimum(&cancellation).await {
            self.persist_cancelled(&job_id)?;
            return Ok(JobCompletion::cancelled());
        }
        self.persist_execution(&job_id, execution, full_result)
    }

    fn spawn_worker(
        &self,
        request: OptimizeRouteRequest,
        cancellation: CancellationToken,
    ) -> (
        tokio::task::JoinHandle<WorkerResult>,
        mpsc::UnboundedReceiver<ExecutionUpdate>,
    ) {
        let (update_tx, update_rx) = mpsc::unbounded_channel();
        let reporter = ProgressForwarder { update_tx };
        let executor = self.executor.clone();
        let analysis_request = request.clone();
        let worker_cancellation = cancellation;
        let execution = tokio::task::spawn_blocking(move || {
            let result = executor.execute(request, &reporter, &worker_cancellation);
            let suggestions = result
                .as_ref()
                .err()
                .map(|error| {
                    executor.failure_suggestions(&analysis_request, error, &worker_cancellation)
                })
                .unwrap_or_default();
            let failure_detail = result
                .as_ref()
                .err()
                .map(|error| service_failure_detail(error, Some(&analysis_request)));
            (result, suggestions, failure_detail)
        });
        (execution, update_rx)
    }

    async fn consume_updates(
        &self,
        job_id: &str,
        pacer: &DebugJobPacer,
        cancellation: &CancellationToken,
        mut updates: mpsc::UnboundedReceiver<ExecutionUpdate>,
    ) -> Option<OptimizeRouteResponse> {
        // The optimizer continues on the blocking pool while this async task
        // exposes its real stage boundaries at the configured target times.
        let mut full_result = None;
        while let Some(update) = updates.recv().await {
            match update {
                ExecutionUpdate::Progress(update) => {
                    if !pacer.wait_for_stage(update.stage, cancellation).await {
                        break;
                    }
                    match self.store.update_progress(
                        job_id,
                        update.stage,
                        update.progress,
                        update.message.as_deref(),
                    ) {
                        Ok(()) => self.publish_persisted(job_id, JobEventKind::Progress),
                        Err(JobStoreError::InvalidTransition {
                            status: JobStatus::Cancelled,
                            ..
                        }) => {
                            cancellation.cancel();
                            break;
                        }
                        Err(error) => {
                            logger().job_operation_failed(
                                "job progress persistence",
                                job_id,
                                &error,
                            );
                        }
                    }
                }
                ExecutionUpdate::Telemetry(event) => {
                    if let Err(error) = self.store.append_solver_event(job_id, &event) {
                        logger().job_operation_failed(
                            "solver telemetry persistence",
                            job_id,
                            &error,
                        );
                    }
                }
                ExecutionUpdate::ResultSnapshot(result) => full_result = Some(result),
            }
        }
        full_result
    }

    fn persist_execution(
        &self,
        job_id: &str,
        execution: Result<WorkerResult, tokio::task::JoinError>,
        full_result: Option<OptimizeRouteResponse>,
    ) -> Result<JobCompletion, JobStoreError> {
        match execution {
            Ok((Ok(result), _, _)) => {
                let result = full_result.unwrap_or(result);
                match self.store.save_result(job_id, &result)? {
                    TerminalWriteOutcome::Applied => {
                        self.publish_persisted(job_id, JobEventKind::Completed);
                        Ok(JobCompletion {
                            result: Ok(result),
                            persisted_error: None,
                        })
                    }
                    TerminalWriteOutcome::Cancelled => Ok(JobCompletion::cancelled()),
                }
            }
            Ok((Err(OptimizationServiceError::Cancelled), _, _)) => {
                self.persist_cancelled(job_id)?;
                Ok(JobCompletion::cancelled())
            }
            Ok((Err(error), suggestions, failure_detail)) => {
                let stored = stored_error(&error, suggestions, failure_detail);
                match self.store.save_error(job_id, &stored)? {
                    TerminalWriteOutcome::Applied => {
                        self.publish_persisted(job_id, JobEventKind::Failed);
                        Ok(JobCompletion {
                            result: Err(error),
                            persisted_error: Some(stored),
                        })
                    }
                    TerminalWriteOutcome::Cancelled => Ok(JobCompletion::cancelled()),
                }
            }
            Err(join_error) => {
                let detail = join_error.to_string();
                let stored = StoredJobError {
                    code: "JOB_EXECUTION_FAILED".to_owned(),
                    message: "Route optimization failed unexpectedly.".to_owned(),
                    detail: detail.clone(),
                    failure_detail: None,
                    suggestions: None,
                };
                match self.store.save_error(job_id, &stored)? {
                    TerminalWriteOutcome::Applied => {
                        self.publish_persisted(job_id, JobEventKind::Failed);
                        Ok(JobCompletion {
                            result: Err(OptimizationServiceError::Solver(SolverError::Failed(
                                detail,
                            ))),
                            persisted_error: Some(stored),
                        })
                    }
                    TerminalWriteOutcome::Cancelled => Ok(JobCompletion::cancelled()),
                }
            }
        }
    }

    fn persist_cancelled(&self, job_id: &str) -> Result<(), JobStoreError> {
        // Usually the cancel endpoint has already persisted this transition.
        // Also cover executors that report cancellation directly.
        match self
            .store
            .cancel_job(job_id, "Optimization cancelled at the user's request.")
        {
            Ok(_) => self.publish_persisted(job_id, JobEventKind::Cancelled),
            Err(JobStoreError::JobNotCancellable { .. }) => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    fn publish_persisted(&self, job_id: &str, kind: JobEventKind) {
        if let Some(activity) = self.cancellations.get(job_id) {
            activity.publish_persisted(self.store.as_ref(), job_id, kind);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JobEventKind {
    Progress,
    Completed,
    Failed,
    Cancelled,
}

impl JobEventKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Progress => "progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct JobEvent {
    pub kind: JobEventKind,
    pub envelope: JobEventEnvelope,
}

#[derive(Clone)]
pub(crate) struct JobActivity {
    cancellation: CancellationToken,
    events: broadcast::Sender<JobEvent>,
    sequence: Arc<AtomicU64>,
}

impl JobActivity {
    fn new(cancellation: CancellationToken) -> Self {
        let (events, _) = broadcast::channel(128);
        Self {
            cancellation,
            events,
            sequence: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();
    }

    fn publish(&self, store: &dyn JobStore, kind: JobEventKind, job: StoredJob) {
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        if let Err(error) = store.record_event_sequence(&job.state.job_id, sequence) {
            logger().job_event_sequence_persistence_failed(&job.state.job_id, sequence, &error);
        }
        let _ = self.events.send(JobEvent {
            kind,
            envelope: JobEventEnvelope::from_stored_job(sequence, job),
        });
    }

    pub(crate) fn publish_persisted(&self, store: &dyn JobStore, job_id: &str, kind: JobEventKind) {
        match store.get_job(job_id) {
            Ok(Some(job)) => {
                if kind == JobEventKind::Progress
                    && !matches!(job.state.status, JobStatus::Pending | JobStatus::Running)
                {
                    return;
                }
                self.publish(store, kind, job);
            }
            Ok(None) => {
                logger().job_event_publish_skipped_missing(job_id);
            }
            Err(error) => {
                logger().job_event_publish_skipped(job_id, &error);
            }
        }
    }

    fn subscribe(&self) -> JobEventSubscription {
        // Subscribe before the HTTP layer reads its stable persisted snapshot
        // so every later event is buffered for live delivery.
        let receiver = self.events.subscribe();
        JobEventSubscription {
            receiver,
            sequence: self.sequence.clone(),
        }
    }
}

pub(crate) struct JobEventSubscription {
    pub receiver: broadcast::Receiver<JobEvent>,
    sequence: Arc<AtomicU64>,
}

impl JobEventSubscription {
    pub(crate) fn current_sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }
}

/// Owns the per-job values whose lifetime spans permit waiting, blocking
/// execution, progress persistence, and terminal state selection.
struct JobExecutionContext {
    request: OptimizeRouteRequest,
    cancellation: CancellationToken,
    pacer: DebugJobPacer,
    job_id: String,
}

impl JobExecutionContext {
    fn new(
        request: OptimizeRouteRequest,
        cancellation: CancellationToken,
        pacer: DebugJobPacer,
    ) -> Self {
        let job_id = request.job_id.clone();
        Self {
            request,
            cancellation,
            pacer,
            job_id,
        }
    }
}

type WorkerResult = (
    Result<OptimizeRouteResponse, OptimizationServiceError>,
    Vec<FailureSuggestion>,
    Option<FailureDetail>,
);

struct ProgressUpdate {
    stage: ProgressStage,
    progress: u8,
    message: Option<String>,
}

enum ExecutionUpdate {
    Progress(ProgressUpdate),
    Telemetry(TelemetryEvent),
    ResultSnapshot(OptimizeRouteResponse),
}

struct ProgressForwarder {
    update_tx: mpsc::UnboundedSender<ExecutionUpdate>,
}

impl OptimizationEventReporter for ProgressForwarder {
    fn progress(&self, stage: ProgressStage, progress: u8, message: Option<&str>) {
        let _ = self
            .update_tx
            .send(ExecutionUpdate::Progress(ProgressUpdate {
                stage,
                progress,
                message: message.map(str::to_owned),
            }));
    }

    // Terminal payloads are written by JobRunner after execute returns. This
    // guarantees payload persistence precedes the terminal state transition.
    fn error(&self, _code: crate::events::OptimizationErrorCode, _message: &str, _detail: &str) {}

    fn result(&self, response: &OptimizeRouteResponse) {
        let _ = self
            .update_tx
            .send(ExecutionUpdate::ResultSnapshot(response.clone()));
    }

    fn solver_telemetry(&self, event: &TelemetryEvent) {
        let _ = self
            .update_tx
            .send(ExecutionUpdate::Telemetry(event.clone()));
    }
}

fn stored_error(
    error: &OptimizationServiceError,
    suggestions: Vec<FailureSuggestion>,
    failure_detail: Option<FailureDetail>,
) -> StoredJobError {
    let (code, message, detail) = match error {
        OptimizationServiceError::Cancelled => (
            "JOB_CANCELLED",
            "The optimization job was cancelled.",
            error.to_string(),
        ),
        OptimizationServiceError::InvalidRequest(source) => (
            "INVALID_REQUEST",
            "The optimize request is invalid.",
            source.to_string(),
        ),
        OptimizationServiceError::Routing(source) => {
            let (code, message) = match source {
                crate::routing::RoutingError::ProviderResolution(_) => (
                    "ROUTE_PROVIDER_RESOLUTION_ERROR",
                    "No route provider could be selected.",
                ),
                crate::routing::RoutingError::ProviderOverrideDisabled => (
                    "PROVIDER_OVERRIDE_DISABLED",
                    "Request-level route provider override is disabled.",
                ),
                crate::routing::RoutingError::ProviderNotConfigured { .. } => (
                    "PROVIDER_NOT_CONFIGURED",
                    "The selected route provider is not configured.",
                ),
                crate::routing::RoutingError::UnsupportedProviderCapability { .. } => (
                    "UNSUPPORTED_PROVIDER_CAPABILITY",
                    "The selected route provider does not support this travel mode.",
                ),
                crate::routing::RoutingError::MatrixBuild { .. } => (
                    "ROUTING_PAIR_FAILED",
                    "A route could not be found for one location pair.",
                ),
                _ => (
                    "ROUTING_UNAVAILABLE",
                    "Travel-time routing is temporarily unavailable.",
                ),
            };
            (code, message, source.to_string())
        }
        OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => match &failure_detail {
            Some(FailureDetail::StartPolicyInfeasible { .. }) => (
                "START_POLICY_INFEASIBLE",
                "The selected start policy cannot produce a feasible route.",
                "the route requires an earlier start".to_owned(),
            ),
            _ => (
                "NO_FEASIBLE_ROUTE",
                "No feasible route was found.",
                "solver found no feasible route".to_owned(),
            ),
        },
        OptimizationServiceError::Schedule(
            source @ (ScheduleError::OutsideSingleDay | ScheduleError::TimeWindowViolation { .. }),
        ) => {
            let (code, message) = match &failure_detail {
                Some(FailureDetail::StartPolicyInfeasible { .. }) => (
                    "START_POLICY_INFEASIBLE",
                    "The selected start policy cannot produce a feasible route.",
                ),
                Some(FailureDetail::TimeWindowViolation { .. }) => (
                    "TIME_WINDOW_VIOLATION",
                    "A location cannot be visited within its opening window.",
                ),
                _ => ("NO_FEASIBLE_ROUTE", "No feasible route was found."),
            };
            (code, message, source.to_string())
        }
        OptimizationServiceError::Solver(source) => (
            "SOLVER_ERROR",
            "Route optimization failed unexpectedly.",
            source.to_string(),
        ),
        OptimizationServiceError::Schedule(source) => (
            "SCHEDULE_ERROR",
            "Route scheduling failed unexpectedly.",
            source.to_string(),
        ),
    };
    StoredJobError {
        code: code.to_owned(),
        message: message.to_owned(),
        detail,
        failure_detail,
        suggestions: (!suggestions.is_empty()).then_some(suggestions),
    }
}

#[cfg(test)]
mod tests;
