use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use serde::Serialize;
use tokio::sync::broadcast;
use tokio::sync::{oneshot, Semaphore};

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse},
    cancellation::CancellationToken,
    events::{OptimizationEventReporter, ProgressStage},
    routing::RoutingProvider,
    schedule::ScheduleError,
    service::{OptimizationServiceError, RouteOptimizationService},
    solver::{RouteSolver, SolverError},
    storage::{
        JobState, JobStatus, JobStore, JobStoreError, StoredJob, StoredJobError,
        TerminalWriteOutcome,
    },
};

/// Executes the optimization pipeline independently of its HTTP request.
pub trait JobExecutor: Send + Sync {
    fn execute(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError>;
}

impl<P, S> JobExecutor for RouteOptimizationService<P, S>
where
    P: RoutingProvider + Send + Sync,
    S: RouteSolver + Send + Sync,
{
    fn execute(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        self.optimize_with_cancellation(request, reporter, cancellation)
    }
}

#[derive(Clone, Default)]
pub(crate) struct JobCancellationRegistry {
    jobs: Arc<Mutex<HashMap<String, JobActivity>>>,
}

impl JobCancellationRegistry {
    fn insert(&self, job_id: &str, activity: JobActivity) {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(job_id.to_owned(), activity);
    }

    pub(crate) fn get(&self, job_id: &str) -> Option<JobActivity> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(job_id)
            .cloned()
    }

    fn remove(&self, job_id: &str) {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(job_id);
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
    pub sequence: u64,
    pub kind: JobEventKind,
    pub data: JobEventData,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct JobEventData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request: Option<OptimizeRouteRequest>,
    pub job_id: String,
    pub status: JobStatus,
    pub stage: Option<ProgressStage>,
    pub progress: u8,
    pub last_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<OptimizeRouteResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<StoredJobError>,
}

impl JobEventData {
    pub(crate) fn snapshot(job: StoredJob) -> Self {
        Self::from_job(job, true)
    }

    fn event(job: StoredJob) -> Self {
        Self::from_job(job, false)
    }

    fn from_job(job: StoredJob, include_request: bool) -> Self {
        Self {
            request: include_request.then_some(job.request),
            job_id: job.state.job_id,
            status: job.state.status,
            stage: job.state.stage,
            progress: job.state.progress,
            last_message: job.state.last_message,
            created_at: job.state.created_at,
            updated_at: job.state.updated_at,
            completed_at: job.state.completed_at,
            result: job.result,
            error: job.error,
        }
    }
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

    fn publish(&self, kind: JobEventKind, data: JobEventData) {
        let sequence = self.sequence.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.events.send(JobEvent {
            sequence,
            kind,
            data,
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
                self.publish(kind, JobEventData::event(job));
            }
            Ok(None) => {
                eprintln!("job event publish skipped; job_id={job_id}; persisted job disappeared")
            }
            Err(error) => {
                eprintln!("job event publish skipped; job_id={job_id}; error={error}")
            }
        }
    }

    fn subscribe(&self) -> JobEventSubscription {
        // Subscribing before reading the cursor ensures every event newer than
        // the cursor is buffered. Events included by the cursor were persisted
        // before publication and will therefore be represented by the snapshot.
        let receiver = self.events.subscribe();
        let cursor = self.sequence.load(Ordering::Acquire);
        JobEventSubscription {
            receiver,
            sequence: self.sequence.clone(),
            cursor,
        }
    }
}

pub(crate) struct JobEventSubscription {
    pub receiver: broadcast::Receiver<JobEvent>,
    sequence: Arc<AtomicU64>,
    pub cursor: u64,
}

impl JobEventSubscription {
    pub(crate) fn current_sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }
}

/// Owns submission and background execution of persisted optimization jobs.
#[derive(Clone)]
pub struct JobRunner {
    executor: Arc<dyn JobExecutor>,
    store: Arc<dyn JobStore>,
    cancellations: JobCancellationRegistry,
    permits: Option<Arc<Semaphore>>,
}

pub(crate) struct JobSubmission {
    pub state: JobState,
    pub completion: oneshot::Receiver<Result<(), String>>,
}

impl JobRunner {
    pub fn new(
        executor: Arc<dyn JobExecutor>,
        store: Arc<dyn JobStore>,
        max_concurrent_jobs: Option<usize>,
    ) -> Self {
        Self {
            executor,
            store,
            cancellations: JobCancellationRegistry::default(),
            permits: max_concurrent_jobs
                .filter(|limit| *limit > 0)
                .map(|limit| Arc::new(Semaphore::new(limit))),
        }
    }

    pub(crate) fn store(&self) -> &Arc<dyn JobStore> {
        &self.store
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
        // Creation is the authoritative duplicate check and happens before any
        // task can observe or execute the job.
        let state = self.store.create_job(&request)?;
        let token = CancellationToken::new();
        self.cancellations
            .insert(&request.job_id, JobActivity::new(token.clone()));

        let (completion_tx, completion) = oneshot::channel();
        let runner = self.clone();
        tokio::spawn(async move {
            let outcome = runner.run(request, token).await;
            let _ = completion_tx.send(outcome.map_err(|error| error.to_string()));
        });

        Ok(JobSubmission { state, completion })
    }

    async fn run(
        &self,
        request: OptimizeRouteRequest,
        cancellation: CancellationToken,
    ) -> Result<(), JobStoreError> {
        let job_id = request.job_id.clone();
        let outcome = self.run_inner(request, cancellation).await;
        self.cancellations.remove(&job_id);
        if let Err(error) = &outcome {
            eprintln!("job execution persistence failed; job_id={job_id}; error={error}");
        }
        outcome
    }

    async fn run_inner(
        &self,
        request: OptimizeRouteRequest,
        cancellation: CancellationToken,
    ) -> Result<(), JobStoreError> {
        let _permit = match &self.permits {
            Some(permits) => Some(
                permits
                    .acquire()
                    .await
                    .expect("job semaphore is not closed"),
            ),
            None => None,
        };
        let job_id = request.job_id.clone();

        // A queued job may have been cancelled while waiting for a permit.
        if cancellation.is_cancelled() {
            return Ok(());
        }
        match self.store.mark_running(&job_id) {
            Ok(()) => {}
            Err(JobStoreError::InvalidTransition {
                status: JobStatus::Cancelled,
                ..
            }) => return Ok(()),
            Err(error) => return Err(error),
        }

        let reporter = PersistedProgressReporter {
            job_id: job_id.clone(),
            store: self.store.clone(),
            cancellation: cancellation.clone(),
            activity: self.cancellations.get(&job_id),
        };
        let executor = self.executor.clone();
        let worker_cancellation = cancellation.clone();
        let execution = tokio::task::spawn_blocking(move || {
            executor.execute(request, &reporter, &worker_cancellation)
        })
        .await;

        match execution {
            Ok(Ok(result)) => {
                if self.store.save_result(&job_id, &result)? == TerminalWriteOutcome::Applied {
                    self.publish_persisted(&job_id, JobEventKind::Completed);
                }
            }
            Ok(Err(OptimizationServiceError::Cancelled)) => {
                // Usually the cancel endpoint has already persisted this
                // transition. Also cover executors that report cancellation
                // directly so no job is left running indefinitely.
                match self
                    .store
                    .cancel_job(&job_id, "Job execution was cancelled")
                {
                    Ok(_) => self.publish_persisted(&job_id, JobEventKind::Cancelled),
                    Err(JobStoreError::JobNotCancellable { .. }) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(Err(error)) => {
                let stored = stored_error(&error);
                if self.store.save_error(&job_id, &stored)? == TerminalWriteOutcome::Applied {
                    self.publish_persisted(&job_id, JobEventKind::Failed);
                }
            }
            Err(join_error) => {
                let stored = StoredJobError {
                    code: "JOB_EXECUTION_FAILED".to_owned(),
                    message: "Route optimization failed unexpectedly.".to_owned(),
                    detail: join_error.to_string(),
                };
                if self.store.save_error(&job_id, &stored)? == TerminalWriteOutcome::Applied {
                    self.publish_persisted(&job_id, JobEventKind::Failed);
                }
            }
        }
        Ok(())
    }

    fn publish_persisted(&self, job_id: &str, kind: JobEventKind) {
        if let Some(activity) = self.cancellations.get(job_id) {
            activity.publish_persisted(self.store.as_ref(), job_id, kind);
        }
    }
}

struct PersistedProgressReporter {
    job_id: String,
    store: Arc<dyn JobStore>,
    cancellation: CancellationToken,
    activity: Option<JobActivity>,
}

impl OptimizationEventReporter for PersistedProgressReporter {
    fn progress(&self, stage: ProgressStage, progress: u8, message: Option<&str>) {
        match self
            .store
            .update_progress(&self.job_id, stage, progress, message)
        {
            Ok(()) => {
                if let Some(activity) = &self.activity {
                    activity.publish_persisted(
                        self.store.as_ref(),
                        &self.job_id,
                        JobEventKind::Progress,
                    );
                }
            }
            Err(
                error @ JobStoreError::InvalidTransition {
                    status: JobStatus::Cancelled,
                    ..
                },
            ) => {
                let _ = error;
                self.cancellation.cancel();
            }
            Err(error) => eprintln!(
                "job progress persistence failed; job_id={}; error={error}",
                self.job_id
            ),
        }
    }

    // Terminal payloads are written by JobRunner after execute returns. This
    // guarantees payload persistence precedes the terminal state transition.
    fn error(&self, _code: crate::events::OptimizationErrorCode, _message: &str, _detail: &str) {}

    fn result(&self, _response: &OptimizeRouteResponse) {}
}

fn stored_error(error: &OptimizationServiceError) -> StoredJobError {
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
        OptimizationServiceError::Routing(source) => (
            "ROUTING_UNAVAILABLE",
            "Travel-time routing is temporarily unavailable.",
            source.to_string(),
        ),
        OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => (
            "NO_FEASIBLE_ROUTE",
            "No feasible route was found.",
            "solver found no feasible route".to_owned(),
        ),
        OptimizationServiceError::Schedule(
            source @ (ScheduleError::OutsideSingleDay | ScheduleError::TimeWindowViolation { .. }),
        ) => (
            "NO_FEASIBLE_ROUTE",
            "No feasible route was found.",
            source.to_string(),
        ),
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
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicU64, Ordering},
            Arc,
        },
        time::Duration,
    };

    use serde_json::json;

    use super::*;
    use crate::storage::FileJobStore;

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "troute-job-events-test-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct FloodExecutor {
        release: Arc<AtomicBool>,
    }

    impl JobExecutor for FloodExecutor {
        fn execute(
            &self,
            _request: OptimizeRouteRequest,
            reporter: &dyn OptimizationEventReporter,
            _cancellation: &CancellationToken,
        ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
            while !self.release.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            for progress in 0..300 {
                reporter.progress(
                    ProgressStage::Solving,
                    (progress % 100) as u8,
                    Some("flooding a slow subscriber"),
                );
            }
            Err(OptimizationServiceError::Solver(SolverError::Failed(
                "expected test failure".to_owned(),
            )))
        }
    }

    fn request() -> OptimizeRouteRequest {
        serde_json::from_value(json!({
            "job_id": "slow-subscriber",
            "locations": [
                {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:59","stay_minutes":0},
                {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:59","stay_minutes":0}
            ],
            "start_time": "09:00"
        }))
        .unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn slow_subscriber_never_blocks_worker_and_can_recover_from_store() {
        let temporary = TestDirectory::new();
        let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
        let release = Arc::new(AtomicBool::new(false));
        let runner = JobRunner::new(
            Arc::new(FloodExecutor {
                release: release.clone(),
            }),
            store.clone(),
            None,
        );
        runner.submit(request()).unwrap();
        let mut subscription = runner.subscribe("slow-subscriber").unwrap();
        release.store(true, Ordering::Release);

        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let job = store.get_job("slow-subscriber").unwrap().unwrap();
                if job.state.status == JobStatus::Failed {
                    assert_eq!(job.error.unwrap().code, "SOLVER_ERROR");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("worker must finish even though the subscriber is not reading");

        assert!(matches!(
            subscription.receiver.recv().await,
            Err(broadcast::error::RecvError::Lagged(_))
        ));
        assert!(subscription.current_sequence() >= 300);
    }
}
