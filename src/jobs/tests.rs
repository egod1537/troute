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
use crate::{
    development::{DevelopmentRouteSolver, DevelopmentRoutingProvider},
    storage::{FileJobStore, JobIndexEntry},
};

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
                ProgressStage::OptimizingRoute,
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
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ],
        "start_time": "09:00"
    }))
    .unwrap()
}

#[test]
fn canonical_event_envelope_snapshot_matches_the_serializer() {
    let suggestion = serde_json::from_value(json!({
        "type": "REMOVE_LOCATION",
        "reason": "Remove the conflicting stop.",
        "confidence": "heuristic",
        "location_id": "B"
    }))
    .unwrap();
    let envelope = JobEventEnvelope::from_stored_job(
        17,
        StoredJob {
            request: request(),
            state: JobState {
                job_id: "wire-contract".to_owned(),
                status: JobStatus::Failed,
                stage: Some(ProgressStage::OptimizingRoute),
                progress: 70,
                last_message: Some("No feasible route was found.".to_owned()),
                created_at: 100,
                updated_at: 123_456_789,
                completed_at: Some(123_456_789),
            },
            result: None,
            preview: None,
            error: Some(StoredJobError {
                code: "NO_FEASIBLE_ROUTE".to_owned(),
                message: "No feasible route was found.".to_owned(),
                detail: "solver found no feasible route".to_owned(),
                failure_detail: Some(FailureDetail::NoFeasibleRoute),
                suggestions: Some(vec![suggestion]),
            }),
        },
    );

    assert!(envelope.is_valid_for_event("snapshot"));
    assert!(envelope.is_valid_for_event("failed"));
    assert_eq!(
        serde_json::to_value(envelope).unwrap(),
        json!({
            "sequence": 17,
            "updated_at": 123_456_789,
            "state": {
                "job_id": "wire-contract",
                "status": "failed",
                "stage": "optimizing_route",
                "progress": 70,
                "last_message": "No feasible route was found.",
                "result": null,
                "error": {
                    "code": "NO_FEASIBLE_ROUTE",
                    "message": "No feasible route was found.",
                    "detail": "solver found no feasible route",
                    "failure_detail": { "type": "NO_FEASIBLE_ROUTE" },
                    "suggestions": [{
                        "type": "REMOVE_LOCATION",
                        "reason": "Remove the conflicting stop.",
                        "confidence": "heuristic",
                        "location_id": "B"
                    }]
                }
            }
        })
    );
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
                assert_eq!(job.state.stage, Some(ProgressStage::OptimizingRoute));
                assert_eq!(job.state.progress, 99);
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

struct RoutingFailureExecutor;

impl JobExecutor for RoutingFailureExecutor {
    fn execute(
        &self,
        _request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        _cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        reporter.progress(
            ProgressStage::FetchingTravelTimes,
            33,
            Some("Fetched 3 / 10 travel-time pairs."),
        );
        Err(OptimizationServiceError::Routing(
            crate::routing::RoutingError::Provider("expected routing failure".to_owned()),
        ))
    }
}

#[tokio::test]
async fn routing_failure_preserves_the_fetching_stage() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let runner = JobRunner::new(Arc::new(RoutingFailureExecutor), store.clone(), None);
    let submission = runner.submit_with_completion(request()).unwrap();

    let completion = submission.completion.await.unwrap().unwrap();
    assert!(matches!(
        completion.result,
        Err(OptimizationServiceError::Routing(
            crate::routing::RoutingError::Provider(ref detail)
        )) if detail == "expected routing failure"
    ));
    let persisted_error = completion.persisted_error.unwrap();
    assert_eq!(persisted_error.code, "ROUTING_UNAVAILABLE");
    assert_eq!(
        persisted_error.failure_detail,
        Some(FailureDetail::RoutingUnavailable)
    );
    assert!(persisted_error.suggestions.is_some());

    let job = store.get_job("slow-subscriber").unwrap().unwrap();
    assert_eq!(job.state.status, JobStatus::Failed);
    assert_eq!(job.state.stage, Some(ProgressStage::FetchingTravelTimes));
    assert_eq!(job.state.progress, 33);
    assert_eq!(job.error, Some(persisted_error));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outstanding_job_capacity_is_released_after_terminal_completion() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let release = Arc::new(AtomicBool::new(false));
    let runner = JobRunner::new_with_limits(
        Arc::new(FloodExecutor {
            release: release.clone(),
        }),
        store,
        Some(1),
        Some(1),
    );
    let mut first = request();
    first.job_id = "capacity-first".to_owned();
    let first = runner.submit_with_completion(first).unwrap();
    let mut rejected = request();
    rejected.job_id = "capacity-rejected".to_owned();

    assert!(matches!(
        runner.submit(rejected),
        Err(JobStoreError::CapacityExceeded { maximum: 1 })
    ));

    release.store(true, Ordering::Release);
    first.completion.await.unwrap().unwrap();
    let mut after_terminal = request();
    after_terminal.job_id = "capacity-after-terminal".to_owned();
    let after_terminal = runner.submit_with_completion(after_terminal).unwrap();
    after_terminal.completion.await.unwrap().unwrap();
}

#[tokio::test]
async fn typed_success_completion_is_sent_after_result_persistence() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let runner = JobRunner::new(
        Arc::new(RouteOptimizationService::new(
            DevelopmentRoutingProvider,
            DevelopmentRouteSolver,
        )),
        store.clone(),
        None,
    );

    let completion = runner
        .submit_with_completion(request())
        .unwrap()
        .completion
        .await
        .unwrap()
        .unwrap();
    assert!(completion.persisted_error.is_none());
    let response = completion.result.unwrap();

    let stored = store.get_job("slow-subscriber").unwrap().unwrap();
    assert_eq!(stored.state.status, JobStatus::Completed);
    assert_eq!(stored.result, Some(response));
}

struct PanickingExecutor;

impl JobExecutor for PanickingExecutor {
    fn execute(
        &self,
        _request: OptimizeRouteRequest,
        _reporter: &dyn OptimizationEventReporter,
        _cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        panic!("simulated worker panic");
    }
}

#[tokio::test]
async fn worker_panic_is_persisted_and_reported_as_typed_completion() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let runner = JobRunner::new(Arc::new(PanickingExecutor), store.clone(), None);

    let completion = runner
        .submit_with_completion(request())
        .unwrap()
        .completion
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        completion.result,
        Err(OptimizationServiceError::Solver(SolverError::Failed(ref detail)))
            if detail.contains("simulated worker panic")
    ));
    let persisted_error = completion.persisted_error.unwrap();
    assert_eq!(persisted_error.code, "JOB_EXECUTION_FAILED");
    assert!(persisted_error.detail.contains("simulated worker panic"));

    let stored = store.get_job("slow-subscriber").unwrap().unwrap();
    assert_eq!(stored.state.status, JobStatus::Failed);
    assert_eq!(stored.error, Some(persisted_error));
}

struct FixedFailureExecutor(OptimizationServiceError);

impl JobExecutor for FixedFailureExecutor {
    fn execute(
        &self,
        _request: OptimizeRouteRequest,
        _reporter: &dyn OptimizationEventReporter,
        _cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        match &self.0 {
            OptimizationServiceError::Cancelled => Err(OptimizationServiceError::Cancelled),
            OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => Err(
                OptimizationServiceError::Solver(SolverError::NoFeasibleRoute),
            ),
            _ => unreachable!("test executor supports only fixed cancellation and solver failure"),
        }
    }
}

#[tokio::test]
async fn solver_failure_completion_preserves_the_service_error() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let runner = JobRunner::new(
        Arc::new(FixedFailureExecutor(OptimizationServiceError::Solver(
            SolverError::NoFeasibleRoute,
        ))),
        store.clone(),
        None,
    );

    let completion = runner
        .submit_with_completion(request())
        .unwrap()
        .completion
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        completion.result,
        Err(OptimizationServiceError::Solver(
            SolverError::NoFeasibleRoute
        ))
    ));
    assert_eq!(
        completion
            .persisted_error
            .as_ref()
            .map(|error| error.code.as_str()),
        Some("NO_FEASIBLE_ROUTE")
    );
    assert_eq!(
        store.get_job("slow-subscriber").unwrap().unwrap().error,
        completion.persisted_error
    );
}

#[tokio::test]
async fn cancellation_completion_matches_the_persisted_terminal_state() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let runner = JobRunner::new(
        Arc::new(FixedFailureExecutor(OptimizationServiceError::Cancelled)),
        store.clone(),
        None,
    );

    let completion = runner
        .submit_with_completion(request())
        .unwrap()
        .completion
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        completion.result,
        Err(OptimizationServiceError::Cancelled)
    ));
    assert!(completion.persisted_error.is_none());
    assert_eq!(
        store
            .get_job("slow-subscriber")
            .unwrap()
            .unwrap()
            .state
            .status,
        JobStatus::Cancelled
    );
}

#[derive(Clone, Copy)]
enum StoreFault {
    FailTerminalWrite,
    PanicWhenMarkingRunning,
}

struct FaultingJobStore {
    inner: FileJobStore,
    fault: StoreFault,
}

impl FaultingJobStore {
    fn new(path: &std::path::Path, fault: StoreFault) -> Self {
        Self {
            inner: FileJobStore::new(path).unwrap(),
            fault,
        }
    }

    fn terminal_error() -> JobStoreError {
        JobStoreError::Io {
            path: PathBuf::from("simulated-terminal-write"),
            source: std::io::Error::other("simulated terminal persistence failure"),
        }
    }
}

impl JobStore for FaultingJobStore {
    fn create_job(&self, request: &OptimizeRouteRequest) -> Result<JobState, JobStoreError> {
        self.inner.create_job(request)
    }

    fn mark_running(&self, job_id: &str) -> Result<(), JobStoreError> {
        match self.fault {
            StoreFault::PanicWhenMarkingRunning => panic!("simulated job task panic"),
            StoreFault::FailTerminalWrite => self.inner.mark_running(job_id),
        }
    }

    fn update_progress(
        &self,
        job_id: &str,
        stage: ProgressStage,
        progress: u8,
        message: Option<&str>,
    ) -> Result<(), JobStoreError> {
        self.inner.update_progress(job_id, stage, progress, message)
    }

    fn save_result(
        &self,
        _job_id: &str,
        _result: &OptimizeRouteResponse,
    ) -> Result<TerminalWriteOutcome, JobStoreError> {
        Err(Self::terminal_error())
    }

    fn save_error(
        &self,
        _job_id: &str,
        _error: &StoredJobError,
    ) -> Result<TerminalWriteOutcome, JobStoreError> {
        Err(Self::terminal_error())
    }

    fn cancel_job(&self, job_id: &str, message: &str) -> Result<JobState, JobStoreError> {
        self.inner.cancel_job(job_id, message)
    }

    fn get_job(&self, job_id: &str) -> Result<Option<StoredJob>, JobStoreError> {
        self.inner.get_job(job_id)
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<JobIndexEntry>, JobStoreError> {
        self.inner.list_recent(limit)
    }

    fn recover_interrupted(&self) -> Result<usize, JobStoreError> {
        self.inner.recover_interrupted()
    }
}

#[tokio::test]
async fn terminal_persistence_failure_is_distinct_from_execution_failure() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FaultingJobStore::new(
        &temporary.0,
        StoreFault::FailTerminalWrite,
    ));
    let runner = JobRunner::new(Arc::new(RoutingFailureExecutor), store, None);

    let completion = runner
        .submit_with_completion(request())
        .unwrap()
        .completion
        .await
        .unwrap();
    assert!(matches!(completion, Err(JobStoreError::Io { .. })));
}

#[tokio::test]
async fn completion_channel_closes_when_the_job_task_panics() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FaultingJobStore::new(
        &temporary.0,
        StoreFault::PanicWhenMarkingRunning,
    ));
    let runner = JobRunner::new(Arc::new(RoutingFailureExecutor), store, None);

    let completion = runner
        .submit_with_completion(request())
        .unwrap()
        .completion
        .await;
    assert!(completion.is_err());
}

struct FastPreviewExecutor;

impl JobExecutor for FastPreviewExecutor {
    fn execute(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        _: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        let mut response =
            RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver)
                .optimize(request)?;
        for improvement in 0..20 {
            response.total_travel_minutes = 100 - improvement;
            reporter.preview(&response);
        }
        reporter.result(&response);
        Ok(response)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fast_job_retains_five_playback_frames_without_delaying_completion_or_next_job() {
    let temporary = TestDirectory::new();
    let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
    let runner = JobRunner::new(Arc::new(FastPreviewExecutor), store.clone(), Some(1));
    tokio::time::timeout(Duration::from_secs(2), async {
        for id in ["first-playback", "second-playback"] {
            let mut input = request();
            input.job_id = id.to_owned();
            runner.submit(input).unwrap();
            loop {
                let job = store.get_job(id).unwrap().unwrap();
                if job.state.status == JobStatus::Completed {
                    let snapshot = JobEventEnvelope::from_stored_job(1, job);
                    assert!(snapshot.is_valid_for_event("completed"));
                    let preview = snapshot.state.preview.unwrap();
                    assert_eq!(preview.frames.len(), 5);
                    assert_eq!(
                        preview
                            .frames
                            .iter()
                            .map(|frame| frame.revision)
                            .collect::<Vec<_>>(),
                        vec![1, 2, 3, 4, 20]
                    );
                    assert_eq!(preview.revision, 20);
                    assert_eq!(preview.result, snapshot.state.result.unwrap());
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    })
    .await
    .expect("frontend playback must not hold server completion or its single Job permit");
}
