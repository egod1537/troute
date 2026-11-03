use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse},
    events::ProgressStage,
    failure::{FailureDetail, FailureSuggestion},
    logging::logger,
    observation::{JobTimelineEntry, JobTimelineStore},
    routing::TravelMode,
    solver::TelemetryEvent,
};

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn is_active(self) -> bool {
        matches!(self, Self::Pending | Self::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalWriteOutcome {
    Applied,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct JobState {
    pub job_id: String,
    pub status: JobStatus,
    pub stage: Option<ProgressStage>,
    pub progress: u8,
    pub last_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct StoredJobError {
    pub code: String,
    pub message: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_detail: Option<FailureDetail>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestions: Option<Vec<FailureSuggestion>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct StoredJob {
    pub request: OptimizeRouteRequest,
    #[serde(flatten)]
    pub state: JobState,
    pub result: Option<OptimizeRouteResponse>,
    pub error: Option<StoredJobError>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct JobIndexEntry {
    pub job_id: String,
    pub status: JobStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub travel_mode: Option<TravelMode>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct JobIndex {
    jobs: Vec<JobIndexEntry>,
}

#[derive(Debug, Error)]
pub enum JobStoreError {
    #[error("job admission capacity is full (maximum outstanding jobs: {maximum})")]
    CapacityExceeded { maximum: usize },
    #[error("job already exists: {0}")]
    DuplicateJob(String),
    #[error("job does not exist: {0}")]
    JobNotFound(String),
    #[error("job is not cancellable because it is already {status:?}: {job_id}")]
    JobNotCancellable { job_id: String, status: JobStatus },
    #[error("job state transition is not allowed from {status:?}: {job_id}")]
    InvalidTransition { job_id: String, status: JobStatus },
    #[error("solver telemetry event has {actual} bytes; maximum is {maximum}")]
    TelemetryEventTooLarge { actual: usize, maximum: usize },
    #[error("job storage I/O failed at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("job storage JSON failed at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

pub trait JobStore: Send + Sync {
    fn create_job(&self, request: &OptimizeRouteRequest) -> Result<JobState, JobStoreError>;
    fn mark_running(&self, job_id: &str) -> Result<(), JobStoreError>;
    fn update_progress(
        &self,
        job_id: &str,
        stage: ProgressStage,
        progress: u8,
        message: Option<&str>,
    ) -> Result<(), JobStoreError>;
    fn save_result(
        &self,
        job_id: &str,
        result: &OptimizeRouteResponse,
    ) -> Result<TerminalWriteOutcome, JobStoreError>;
    fn save_error(
        &self,
        job_id: &str,
        error: &StoredJobError,
    ) -> Result<TerminalWriteOutcome, JobStoreError>;
    fn cancel_job(&self, job_id: &str, message: &str) -> Result<JobState, JobStoreError>;
    fn get_job(&self, job_id: &str) -> Result<Option<StoredJob>, JobStoreError>;
    fn list_recent(&self, limit: usize) -> Result<Vec<JobIndexEntry>, JobStoreError>;
    fn recover_interrupted(&self) -> Result<usize, JobStoreError>;

    fn append_solver_event(
        &self,
        _job_id: &str,
        _event: &TelemetryEvent,
    ) -> Result<(), JobStoreError> {
        Ok(())
    }

    fn list_solver_events(
        &self,
        _job_id: &str,
        _after: u64,
        _limit: usize,
    ) -> Result<Vec<TelemetryEvent>, JobStoreError> {
        Ok(Vec::new())
    }

    /// Returns the last SSE sequence persisted for this Job. `None` supports
    /// custom stores and records created before sequence persistence existed.
    fn current_event_sequence(&self, _job_id: &str) -> Result<Option<u64>, JobStoreError> {
        Ok(None)
    }

    /// Persists SSE sequence metadata without adding it to the Job detail DTO.
    fn record_event_sequence(&self, _job_id: &str, _sequence: u64) -> Result<(), JobStoreError> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct FileJobStore {
    data_dir: PathBuf,
    lock: Mutex<()>,
}

impl FileJobStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Result<Self, JobStoreError> {
        let store = Self {
            data_dir: data_dir.into(),
            lock: Mutex::new(()),
        };
        store.ensure_directories()?;
        Ok(store)
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    fn jobs_dir(&self) -> PathBuf {
        self.data_dir.join("jobs")
    }

    fn job_dir(&self, job_id: &str) -> PathBuf {
        self.jobs_dir().join(encode_job_id(job_id))
    }

    fn ensure_directories(&self) -> Result<(), JobStoreError> {
        let path = self.jobs_dir();
        fs::create_dir_all(&path).map_err(|source| JobStoreError::Io { path, source })
    }

    fn state_path(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("state.json")
    }

    fn event_sequence_path(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("event-sequence.json")
    }

    fn solver_telemetry_path(&self, job_id: &str) -> PathBuf {
        self.job_dir(job_id).join("solver-telemetry.jsonl")
    }

    fn read_state(&self, job_id: &str) -> Result<JobState, JobStoreError> {
        read_json(&self.state_path(job_id))
    }

    fn write_state(&self, state: &JobState) -> Result<(), JobStoreError> {
        atomic_write_json(&self.state_path(&state.job_id), state)
    }

    fn mutate_state<T>(
        &self,
        job_id: &str,
        mutate: impl FnOnce(&mut JobState) -> Result<T, JobStoreError>,
    ) -> Result<T, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut state = self.read_state(job_id)?;
        let output = mutate(&mut state)?;
        state.updated_at = now_ms();
        self.write_state(&state)?;
        self.update_index_entry(&state)?;
        Ok(output)
    }

    fn index_path(&self) -> PathBuf {
        self.data_dir.join("index.json")
    }

    fn read_index(&self) -> Result<JobIndex, JobStoreError> {
        let path = self.index_path();
        if !path.exists() {
            return Ok(JobIndex::default());
        }
        read_json(&path)
    }

    fn update_index_entry(&self, state: &JobState) -> Result<(), JobStoreError> {
        let request: OptimizeRouteRequest =
            read_json(&self.job_dir(&state.job_id).join("request.json"))?;
        let mut index = self.read_index()?;
        index.jobs.retain(|entry| entry.job_id != state.job_id);
        index
            .jobs
            .insert(0, index_entry(state, request.travel_mode));
        sort_index(&mut index.jobs);
        atomic_write_json(&self.index_path(), &index)
    }

    fn rebuild_index(&self) -> Result<(), JobStoreError> {
        let mut jobs = Vec::new();
        for directory in read_directories(&self.jobs_dir())? {
            let state_path = directory.join("state.json");
            if state_path.is_file() {
                let state: JobState = read_json(&state_path)?;
                let request: OptimizeRouteRequest = read_json(&directory.join("request.json"))?;
                jobs.push(index_entry(&state, request.travel_mode));
            }
        }
        sort_index(&mut jobs);
        atomic_write_json(&self.index_path(), &JobIndex { jobs })
    }

    /// Validates the shared precondition for result/error terminal writes.
    /// Callers hold the store lock across this check, payload persistence, and
    /// the final state/index update so cancellation cannot interleave.
    fn prepare_terminal_write(
        &self,
        job_id: &str,
    ) -> Result<Option<(PathBuf, JobState)>, JobStoreError> {
        let directory = self.job_dir(job_id);
        if !directory.exists() {
            return Err(JobStoreError::JobNotFound(job_id.to_owned()));
        }
        let state = self.read_state(job_id)?;
        if state.status == JobStatus::Cancelled {
            return Ok(None);
        }
        if state.status != JobStatus::Running {
            return Err(JobStoreError::InvalidTransition {
                job_id: job_id.to_owned(),
                status: state.status,
            });
        }
        Ok(Some((directory, state)))
    }

    fn commit_terminal_state(
        &self,
        mut state: JobState,
        status: JobStatus,
    ) -> Result<(), JobStoreError> {
        let timestamp = now_ms();
        state.status = status;
        if status == JobStatus::Completed {
            state.progress = 100;
        }
        state.updated_at = timestamp;
        state.completed_at = Some(timestamp);
        self.write_state(&state)?;
        self.update_index_entry(&state)
    }
}

impl JobStore for FileJobStore {
    fn create_job(&self, request: &OptimizeRouteRequest) -> Result<JobState, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_directories()?;
        let job_dir = self.job_dir(&request.job_id);
        match fs::create_dir(&job_dir) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(JobStoreError::DuplicateJob(request.job_id.clone()));
            }
            Err(source) => {
                return Err(JobStoreError::Io {
                    path: job_dir,
                    source,
                });
            }
        }

        atomic_write_json(&job_dir.join("request.json"), request)?;
        atomic_write_json(&job_dir.join("event-sequence.json"), &0_u64)?;
        let timestamp = now_ms();
        let state = JobState {
            job_id: request.job_id.clone(),
            status: JobStatus::Pending,
            stage: None,
            progress: 0,
            last_message: None,
            created_at: timestamp,
            updated_at: timestamp,
            completed_at: None,
        };
        self.write_state(&state)?;
        self.update_index_entry(&state)?;
        Ok(state)
    }

    fn mark_running(&self, job_id: &str) -> Result<(), JobStoreError> {
        self.mutate_state(job_id, |state| {
            if state.status.is_active() {
                state.status = JobStatus::Running;
                Ok(())
            } else {
                Err(JobStoreError::InvalidTransition {
                    job_id: job_id.to_owned(),
                    status: state.status,
                })
            }
        })
    }

    fn update_progress(
        &self,
        job_id: &str,
        stage: ProgressStage,
        progress: u8,
        message: Option<&str>,
    ) -> Result<(), JobStoreError> {
        self.mutate_state(job_id, |state| {
            if !state.status.is_active() {
                return Err(JobStoreError::InvalidTransition {
                    job_id: job_id.to_owned(),
                    status: state.status,
                });
            }
            if state.stage.is_some_and(|current| stage < current) {
                return Ok(());
            }
            state.status = JobStatus::Running;
            state.stage = Some(stage);
            state.progress = state.progress.max(progress.min(100));
            state.last_message = message.map(str::to_owned);
            Ok(())
        })
    }

    fn save_result(
        &self,
        job_id: &str,
        result: &OptimizeRouteResponse,
    ) -> Result<TerminalWriteOutcome, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((directory, state)) = self.prepare_terminal_write(job_id)? else {
            return Ok(TerminalWriteOutcome::Cancelled);
        };
        let path = directory.join("result.json");
        atomic_write_json(&path, result)?;
        self.commit_terminal_state(state, JobStatus::Completed)?;
        Ok(TerminalWriteOutcome::Applied)
    }

    fn save_error(
        &self,
        job_id: &str,
        error: &StoredJobError,
    ) -> Result<TerminalWriteOutcome, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((directory, state)) = self.prepare_terminal_write(job_id)? else {
            return Ok(TerminalWriteOutcome::Cancelled);
        };
        let path = directory.join("error.json");
        atomic_write_json(&path, error)?;
        self.commit_terminal_state(state, JobStatus::Failed)?;
        Ok(TerminalWriteOutcome::Applied)
    }

    fn cancel_job(&self, job_id: &str, message: &str) -> Result<JobState, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = self.job_dir(job_id);
        if !directory.is_dir() {
            return Err(JobStoreError::JobNotFound(job_id.to_owned()));
        }
        let mut state = self.read_state(job_id)?;
        if !state.status.is_active() {
            return Err(JobStoreError::JobNotCancellable {
                job_id: job_id.to_owned(),
                status: state.status,
            });
        }
        let timestamp = now_ms();
        state.status = JobStatus::Cancelled;
        state.last_message = Some(message.to_owned());
        state.updated_at = timestamp;
        state.completed_at = Some(timestamp);
        self.write_state(&state)?;
        self.update_index_entry(&state)?;
        Ok(state)
    }

    fn get_job(&self, job_id: &str) -> Result<Option<StoredJob>, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = self.job_dir(job_id);
        if !directory.is_dir() {
            return Ok(None);
        }
        Ok(Some(StoredJob {
            request: read_json(&directory.join("request.json"))?,
            state: read_json(&directory.join("state.json"))?,
            result: read_optional_json(&directory.join("result.json"))?,
            error: read_optional_json(&directory.join("error.json"))?,
        }))
    }

    fn list_recent(&self, limit: usize) -> Result<Vec<JobIndexEntry>, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut jobs = self.read_index()?.jobs;
        sort_index(&mut jobs);
        jobs.truncate(limit);
        Ok(jobs)
    }

    fn recover_interrupted(&self) -> Result<usize, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.ensure_directories()?;
        let mut recovered = 0;
        for directory in read_directories(&self.jobs_dir())? {
            let state_path = directory.join("state.json");
            if !state_path.is_file() {
                continue;
            }
            let mut state: JobState = read_json(&state_path)?;
            if matches!(state.status, JobStatus::Pending | JobStatus::Running) {
                let error = StoredJobError {
                    code: "JOB_INTERRUPTED".to_owned(),
                    message: "Job was interrupted by troute restart".to_owned(),
                    detail: "troute restarted before the job reached a terminal state".to_owned(),
                    failure_detail: None,
                    suggestions: None,
                };
                atomic_write_json(&directory.join("error.json"), &error)?;
                let timestamp = now_ms();
                state.status = JobStatus::Failed;
                state.updated_at = timestamp;
                state.completed_at = Some(timestamp);
                atomic_write_json(&state_path, &state)?;
                let sequence_path = directory.join("event-sequence.json");
                let sequence = read_optional_json::<u64>(&sequence_path)?
                    .unwrap_or(0)
                    .saturating_add(1);
                atomic_write_json(&sequence_path, &sequence)?;
                recovered += 1;
            }
        }
        self.rebuild_index()?;
        Ok(recovered)
    }

    fn append_solver_event(
        &self,
        job_id: &str,
        event: &TelemetryEvent,
    ) -> Result<(), JobStoreError> {
        const MAX_EVENT_BYTES: usize = 64 * 1024;
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = self.job_dir(job_id);
        if !directory.is_dir() {
            return Err(JobStoreError::JobNotFound(job_id.to_owned()));
        }
        let path = self.solver_telemetry_path(job_id);
        let serialized = serde_json::to_vec(event).map_err(|source| JobStoreError::Json {
            path: path.clone(),
            source,
        })?;
        if serialized.len() > MAX_EVENT_BYTES {
            return Err(JobStoreError::TelemetryEventTooLarge {
                actual: serialized.len(),
                maximum: MAX_EVENT_BYTES,
            });
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| JobStoreError::Io {
                path: path.clone(),
                source,
            })?;
        let mut writer = BufWriter::new(file);
        writer
            .write_all(&serialized)
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush())
            .map_err(|source| JobStoreError::Io { path, source })
    }

    fn list_solver_events(
        &self,
        job_id: &str,
        after: u64,
        limit: usize,
    ) -> Result<Vec<TelemetryEvent>, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = self.job_dir(job_id);
        if !directory.is_dir() {
            return Err(JobStoreError::JobNotFound(job_id.to_owned()));
        }
        let path = self.solver_telemetry_path(job_id);
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(JobStoreError::Io { path, source }),
        };
        let mut events = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = match line {
                Ok(line) => line,
                Err(source) => return Err(JobStoreError::Io { path, source }),
            };
            if line.trim().is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<TelemetryEvent>(&line) else {
                // A crash may leave the final JSONL line incomplete. Earlier
                // complete events remain authoritative.
                continue;
            };
            if event.event_sequence > after {
                events.push(event);
                if events.len() >= limit.max(1) {
                    break;
                }
            }
        }
        Ok(events)
    }

    fn current_event_sequence(&self, job_id: &str) -> Result<Option<u64>, JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        read_optional_json(&self.event_sequence_path(job_id))
    }

    fn record_event_sequence(&self, job_id: &str, sequence: u64) -> Result<(), JobStoreError> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let directory = self.job_dir(job_id);
        if !directory.exists() {
            return Err(JobStoreError::JobNotFound(job_id.to_owned()));
        }
        atomic_write_json(&self.event_sequence_path(job_id), &sequence)
    }
}

#[derive(Debug)]
pub struct FileJobTimelineStore {
    data_dir: PathBuf,
    lock: Mutex<()>,
}

impl FileJobTimelineStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Result<Self, JobStoreError> {
        let store = Self {
            data_dir: data_dir.into(),
            lock: Mutex::new(()),
        };
        let path = store.data_dir.join("jobs");
        fs::create_dir_all(&path).map_err(|source| JobStoreError::Io { path, source })?;
        Ok(store)
    }

    fn timeline_path(&self, job_id: &str) -> PathBuf {
        self.data_dir
            .join("jobs")
            .join(encode_job_id(job_id))
            .join("timeline.jsonl")
    }
}

impl JobTimelineStore for FileJobTimelineStore {
    fn append(&self, job_id: &str, entry: JobTimelineEntry) {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.timeline_path(job_id);
        let result = (|| -> Result<(), JobStoreError> {
            let parent = path.parent().expect("timeline path has a parent");
            fs::create_dir_all(parent).map_err(|source| JobStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
            let serialized = serde_json::to_vec(&entry).map_err(|source| JobStoreError::Json {
                path: path.clone(),
                source,
            })?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|source| JobStoreError::Io {
                    path: path.clone(),
                    source,
                })?;
            let mut writer = BufWriter::new(file);
            writer
                .write_all(&serialized)
                .and_then(|()| writer.write_all(b"\n"))
                .and_then(|()| writer.flush())
                .map_err(|source| JobStoreError::Io {
                    path: path.clone(),
                    source,
                })
        })();
        if let Err(error) = result {
            logger().job_operation_failed("job timeline append", job_id, &error);
        }
    }

    fn list(&self, job_id: &str) -> Vec<JobTimelineEntry> {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.timeline_path(job_id);
        let file = match File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => {
                logger().job_operation_failed("job timeline read", job_id, &error);
                return Vec::new();
            }
        };
        BufReader::new(file)
            .lines()
            .filter_map(|line| match line {
                Ok(line) if line.trim().is_empty() => None,
                Ok(line) => match serde_json::from_str(&line) {
                    Ok(entry) => Some(entry),
                    Err(error) => {
                        logger().malformed_timeline_line(job_id, &error);
                        None
                    }
                },
                Err(error) => {
                    logger().job_operation_failed("job timeline line read", job_id, &error);
                    None
                }
            })
            .collect()
    }

    fn clear(&self, job_id: &str) {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = self.timeline_path(job_id);
        if let Err(error) = fs::remove_file(&path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                logger().job_operation_failed("job timeline clear", job_id, &error);
            }
        }
    }
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<(), JobStoreError> {
    let parent = path.parent().expect("JSON path has a parent");
    fs::create_dir_all(parent).map_err(|source| JobStoreError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let suffix = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("data.json");
    let temporary = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        suffix
    ));
    let result = (|| -> Result<(), JobStoreError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| JobStoreError::Io {
                path: temporary.clone(),
                source,
            })?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, value).map_err(|source| JobStoreError::Json {
            path: temporary.clone(),
            source,
        })?;
        writer
            .write_all(b"\n")
            .and_then(|()| writer.flush())
            .map_err(|source| JobStoreError::Io {
                path: temporary.clone(),
                source,
            })?;
        fs::rename(&temporary, path).map_err(|source| JobStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, JobStoreError> {
    let file = File::open(path).map_err(|source| JobStoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_reader(BufReader::new(file)).map_err(|source| JobStoreError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn read_optional_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, JobStoreError> {
    if !path.exists() {
        return Ok(None);
    }
    read_json(path).map(Some)
}

fn read_directories(path: &Path) -> Result<Vec<PathBuf>, JobStoreError> {
    let entries = fs::read_dir(path).map_err(|source| JobStoreError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut directories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| JobStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| JobStoreError::Io {
            path: entry.path(),
            source,
        })?;
        if file_type.is_dir() {
            directories.push(entry.path());
        }
    }
    Ok(directories)
}

fn index_entry(state: &JobState, travel_mode: Option<TravelMode>) -> JobIndexEntry {
    JobIndexEntry {
        job_id: state.job_id.clone(),
        status: state.status,
        travel_mode,
        created_at: state.created_at,
        updated_at: state.updated_at,
    }
}

fn sort_index(jobs: &mut [JobIndexEntry]) {
    jobs.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
}

fn encode_job_id(job_id: &str) -> String {
    let mut encoded = String::with_capacity(job_id.len());
    for byte in job_id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn now_ms() -> i64 {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(milliseconds).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests;
