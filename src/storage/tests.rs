use super::*;
use crate::observation::{JobObservationRecorder, ObservationDirection, ObservationPeer};
use serde_json::json;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "troute-storage-test-{}-{}",
            std::process::id(),
            NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed)
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

fn request(job_id: &str) -> OptimizeRouteRequest {
    serde_json::from_value(json!({
            "job_id": job_id,
            "locations": [
                {"id":"A","place_id":"place-a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"B","place_id":"place-b","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ],
            "start_time": "09:00"
        }))
        .unwrap()
}

fn response() -> OptimizeRouteResponse {
    serde_json::from_value(json!({
        "route": [
            {"location_id":"A","order":0,"arrival_time":"09:00","departure_time":"09:00"},
            {"location_id":"B","order":1,"arrival_time":"09:10"}
        ],
        "total_travel_minutes": 10
    }))
    .unwrap()
}

#[test]
fn creates_data_dir_and_persists_request_state_progress_and_result() {
    let temporary = TestDirectory::new();
    let data_dir = temporary.0.join("nested/data");
    let store = FileJobStore::new(&data_dir).unwrap();
    let initial = store.create_job(&request("job-001")).unwrap();
    assert_eq!(initial.status, JobStatus::Pending);
    assert!(data_dir.join("jobs/job-001/request.json").is_file());

    store.mark_running("job-001").unwrap();
    store
        .update_progress(
            "job-001",
            ProgressStage::OptimizingRoute,
            65,
            Some("Optimizing route"),
        )
        .unwrap();
    store.save_result("job-001", &response()).unwrap();

    let job = store.get_job("job-001").unwrap().unwrap();
    assert_eq!(job.state.status, JobStatus::Completed);
    assert_eq!(job.state.progress, 100);
    assert_eq!(job.result, Some(response()));
    assert!(data_dir.join("index.json").is_file());
}

#[test]
fn saves_error_rejects_duplicates_and_encodes_unsafe_job_ids() {
    let temporary = TestDirectory::new();
    let store = FileJobStore::new(&temporary.0).unwrap();
    let input = request("../job/unsafe");
    store.create_job(&input).unwrap();
    assert!(matches!(
        store.create_job(&input),
        Err(JobStoreError::DuplicateJob(_))
    ));
    store.mark_running(&input.job_id).unwrap();
    store
        .save_error(
            &input.job_id,
            &StoredJobError {
                code: "TEST_ERROR".to_owned(),
                message: "failed".to_owned(),
                detail: "detail".to_owned(),
                failure_detail: None,
                suggestions: None,
            },
        )
        .unwrap();
    let job = store.get_job(&input.job_id).unwrap().unwrap();
    assert_eq!(job.state.status, JobStatus::Failed);
    assert_eq!(job.error.unwrap().code, "TEST_ERROR");
    assert!(!temporary.0.parent().unwrap().join("job").exists());
}

#[test]
fn stored_errors_without_failure_extensions_remain_compatible() {
    let error: StoredJobError = serde_json::from_value(serde_json::json!({
        "code": "LEGACY_ERROR",
        "message": "legacy failure",
        "detail": "written before suggestions were added"
    }))
    .unwrap();

    assert_eq!(error.code, "LEGACY_ERROR");
    assert!(error.failure_detail.is_none());
    assert!(error.suggestions.is_none());
}

#[test]
fn restart_marks_only_non_terminal_jobs_failed() {
    let temporary = TestDirectory::new();
    let store = FileJobStore::new(&temporary.0).unwrap();
    store.create_job(&request("pending")).unwrap();
    store.create_job(&request("completed")).unwrap();
    store.mark_running("completed").unwrap();
    store.save_result("completed", &response()).unwrap();
    drop(store);

    let reopened = FileJobStore::new(&temporary.0).unwrap();
    assert_eq!(reopened.recover_interrupted().unwrap(), 1);
    let pending = reopened.get_job("pending").unwrap().unwrap();
    assert_eq!(pending.state.status, JobStatus::Failed);
    assert_eq!(pending.error.unwrap().code, "JOB_INTERRUPTED");
    assert_eq!(
        reopened.get_job("completed").unwrap().unwrap().state.status,
        JobStatus::Completed
    );
}

#[test]
fn pending_and_running_jobs_can_be_cancelled_without_error_files() {
    let temporary = TestDirectory::new();
    let store = FileJobStore::new(&temporary.0).unwrap();
    store.create_job(&request("pending-cancel")).unwrap();
    let pending = store
        .cancel_job("pending-cancel", "Job cancelled by request")
        .unwrap();
    assert_eq!(pending.status, JobStatus::Cancelled);
    assert!(pending.completed_at.is_some());
    assert_eq!(pending.progress, 0);

    store.create_job(&request("running-cancel")).unwrap();
    store.mark_running("running-cancel").unwrap();
    store
        .update_progress(
            "running-cancel",
            ProgressStage::OptimizingRoute,
            65,
            Some("Optimizing route"),
        )
        .unwrap();
    let running = store
        .cancel_job("running-cancel", "Job cancelled by request")
        .unwrap();
    assert_eq!(running.status, JobStatus::Cancelled);
    assert_eq!(running.progress, 65);
    assert_eq!(running.stage, Some(ProgressStage::OptimizingRoute));
    assert_eq!(
        running.last_message.as_deref(),
        Some("Job cancelled by request")
    );
    assert!(!temporary.0.join("jobs/running-cancel/error.json").exists());
    assert!(!temporary.0.join("jobs/running-cancel/result.json").exists());
}

#[test]
fn terminal_jobs_reject_cancellation_and_cancelled_survives_restart() {
    let temporary = TestDirectory::new();
    let store = FileJobStore::new(&temporary.0).unwrap();

    store.create_job(&request("completed")).unwrap();
    store.mark_running("completed").unwrap();
    store.save_result("completed", &response()).unwrap();
    store.create_job(&request("failed")).unwrap();
    store.mark_running("failed").unwrap();
    store
        .save_error(
            "failed",
            &StoredJobError {
                code: "TEST".to_owned(),
                message: "failed".to_owned(),
                detail: "detail".to_owned(),
                failure_detail: None,
                suggestions: None,
            },
        )
        .unwrap();
    store.create_job(&request("cancelled")).unwrap();
    store
        .cancel_job("cancelled", "Job cancelled by request")
        .unwrap();

    for (job_id, expected) in [
        ("completed", JobStatus::Completed),
        ("failed", JobStatus::Failed),
        ("cancelled", JobStatus::Cancelled),
    ] {
        assert!(matches!(
            store.cancel_job(job_id, "again"),
            Err(JobStoreError::JobNotCancellable { status, .. }) if status == expected
        ));
    }
    drop(store);

    let reopened = FileJobStore::new(&temporary.0).unwrap();
    assert_eq!(reopened.recover_interrupted().unwrap(), 0);
    assert_eq!(
        reopened.get_job("cancelled").unwrap().unwrap().state.status,
        JobStatus::Cancelled
    );
}

#[test]
fn result_and_cancel_race_has_exactly_one_terminal_winner() {
    use std::sync::{Arc, Barrier};

    for iteration in 0..20 {
        let temporary = TestDirectory::new();
        let store = Arc::new(FileJobStore::new(&temporary.0).unwrap());
        let job_id = format!("race-{iteration}");
        store.create_job(&request(&job_id)).unwrap();
        store.mark_running(&job_id).unwrap();
        let barrier = Arc::new(Barrier::new(3));

        let result_store = store.clone();
        let result_barrier = barrier.clone();
        let result_job_id = job_id.clone();
        let result_thread = std::thread::spawn(move || {
            result_barrier.wait();
            result_store.save_result(&result_job_id, &response())
        });

        let cancel_store = store.clone();
        let cancel_barrier = barrier.clone();
        let cancel_job_id = job_id.clone();
        let cancel_thread = std::thread::spawn(move || {
            cancel_barrier.wait();
            cancel_store.cancel_job(&cancel_job_id, "Job cancelled by request")
        });

        barrier.wait();
        let result_outcome = result_thread.join().unwrap();
        let cancel_outcome = cancel_thread.join().unwrap();
        let job = store.get_job(&job_id).unwrap().unwrap();
        match job.state.status {
            JobStatus::Completed => {
                assert_eq!(result_outcome.unwrap(), TerminalWriteOutcome::Applied);
                assert!(matches!(
                    cancel_outcome,
                    Err(JobStoreError::JobNotCancellable {
                        status: JobStatus::Completed,
                        ..
                    })
                ));
                assert!(job.result.is_some());
            }
            JobStatus::Cancelled => {
                assert_eq!(result_outcome.unwrap(), TerminalWriteOutcome::Cancelled);
                assert!(cancel_outcome.is_ok());
                assert!(job.result.is_none());
            }
            status => panic!("unexpected race winner state: {status:?}"),
        }
    }
}

#[test]
fn timeline_is_append_only_json_lines_and_survives_reopen() {
    let temporary = TestDirectory::new();
    let store = FileJobTimelineStore::new(&temporary.0).unwrap();
    let recorder = JobObservationRecorder::new(std::sync::Arc::new(store));
    let mut first = recorder.entry(
        "pair-1",
        ObservationDirection::Request,
        ObservationPeer::Testbed,
        ObservationPeer::Troute,
    );
    first.method = Some("POST".to_owned());
    recorder.append("job-1", first);
    recorder.append(
        "job-1",
        recorder.entry(
            "pair-1",
            ObservationDirection::Response,
            ObservationPeer::Troute,
            ObservationPeer::Testbed,
        ),
    );

    let reopened = JobObservationRecorder::new(std::sync::Arc::new(
        FileJobTimelineStore::new(&temporary.0).unwrap(),
    ));
    assert_eq!(reopened.list("job-1").len(), 2);
    let raw = fs::read_to_string(temporary.0.join("jobs/job-1/timeline.jsonl")).unwrap();
    assert_eq!(raw.lines().count(), 2);
}

#[test]
fn runtime_data_directories_are_ignored_by_git() {
    let ignore =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|line| line == ".local/"));
    assert!(ignore.lines().any(|line| line == "runtime/"));
}

#[test]
fn live_preview_is_provisional_replaced_and_cannot_overwrite_terminal_state() {
    use crate::events::OptimizationPreview;
    let temporary = TestDirectory::new();
    let store = FileJobStore::new(&temporary.0).unwrap();
    store.create_job(&request("preview")).unwrap();
    store.mark_running("preview").unwrap();
    for revision in 1..=10 {
        assert!(store
            .save_preview(
                "preview",
                &OptimizationPreview {
                    revision,
                    result: response()
                }
            )
            .unwrap());
    }
    let active = store.get_job("preview").unwrap().unwrap();
    assert_eq!(active.state.status, JobStatus::Running);
    assert!(active.result.is_none());
    assert_eq!(active.preview.unwrap().revision, 10);
    let snapshot = crate::jobs::JobEventEnvelope::from_stored_job(
        10,
        store.get_job("preview").unwrap().unwrap(),
    );
    assert!(snapshot.is_valid_for_event("snapshot"));
    assert_eq!(snapshot.state.preview.unwrap().revision, 10);
    store.cancel_job("preview", "cancelled").unwrap();
    assert!(!store
        .save_preview(
            "preview",
            &OptimizationPreview {
                revision: 11,
                result: response()
            }
        )
        .unwrap());
    let terminal = crate::jobs::JobEventEnvelope::from_stored_job(
        11,
        store.get_job("preview").unwrap().unwrap(),
    );
    assert!(terminal.is_valid_for_event("cancelled"));
    assert!(terminal.state.preview.is_none());
    assert!(terminal.state.result.is_none());
}
