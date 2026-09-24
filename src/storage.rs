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
    observation::{JobTimelineEntry, JobTimelineStore},
    routing::TravelMode,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
    #[error("job already exists: {0}")]
    DuplicateJob(String),
    #[error("job does not exist: {0}")]
    JobNotFound(String),
    #[error("job is not cancellable because it is already {status:?}: {job_id}")]
    JobNotCancellable { job_id: String, status: JobStatus },
    #[error("job state transition is not allowed from {status:?}: {job_id}")]
    InvalidTransition { job_id: String, status: JobStatus },
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
            state.status = JobStatus::Running;
            state.stage = Some(stage);
            state.progress = progress;
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
        let directory = self.job_dir(job_id);
        if !directory.exists() {
            return Err(JobStoreError::JobNotFound(job_id.to_owned()));
        }
        let mut state = self.read_state(job_id)?;
        if state.status == JobStatus::Cancelled {
            return Ok(TerminalWriteOutcome::Cancelled);
        }
        if state.status != JobStatus::Running {
            return Err(JobStoreError::InvalidTransition {
                job_id: job_id.to_owned(),
                status: state.status,
            });
        }
        let path = directory.join("result.json");
        atomic_write_json(&path, result)?;
        let timestamp = now_ms();
        state.status = JobStatus::Completed;
        state.progress = 100;
        state.updated_at = timestamp;
        state.completed_at = Some(timestamp);
        self.write_state(&state)?;
        self.update_index_entry(&state)?;
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
        let directory = self.job_dir(job_id);
        if !directory.exists() {
            return Err(JobStoreError::JobNotFound(job_id.to_owned()));
        }
        let mut state = self.read_state(job_id)?;
        if state.status == JobStatus::Cancelled {
            return Ok(TerminalWriteOutcome::Cancelled);
        }
        if state.status != JobStatus::Running {
            return Err(JobStoreError::InvalidTransition {
                job_id: job_id.to_owned(),
                status: state.status,
            });
        }
        let path = directory.join("error.json");
        atomic_write_json(&path, error)?;
        let timestamp = now_ms();
        state.status = JobStatus::Failed;
        state.updated_at = timestamp;
        state.completed_at = Some(timestamp);
        self.write_state(&state)?;
        self.update_index_entry(&state)?;
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
                };
                atomic_write_json(&directory.join("error.json"), &error)?;
                let timestamp = now_ms();
                state.status = JobStatus::Failed;
                state.updated_at = timestamp;
                state.completed_at = Some(timestamp);
                atomic_write_json(&state_path, &state)?;
                recovered += 1;
            }
        }
        self.rebuild_index()?;
        Ok(recovered)
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
            eprintln!("job timeline append failed; job_id={job_id}; error={error}");
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
                eprintln!("job timeline read failed; job_id={job_id}; error={error}");
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
                        eprintln!(
                            "ignoring malformed timeline line; job_id={job_id}; error={error}"
                        );
                        None
                    }
                },
                Err(error) => {
                    eprintln!("job timeline line read failed; job_id={job_id}; error={error}");
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
                eprintln!("job timeline clear failed; job_id={job_id}; error={error}");
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
mod tests {
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
            .update_progress("job-001", ProgressStage::Solving, 60, Some("Solving route"))
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
                },
            )
            .unwrap();
        let job = store.get_job(&input.job_id).unwrap().unwrap();
        assert_eq!(job.state.status, JobStatus::Failed);
        assert_eq!(job.error.unwrap().code, "TEST_ERROR");
        assert!(!temporary.0.parent().unwrap().join("job").exists());
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
                ProgressStage::Solving,
                60,
                Some("Solving route"),
            )
            .unwrap();
        let running = store
            .cancel_job("running-cancel", "Job cancelled by request")
            .unwrap();
        assert_eq!(running.status, JobStatus::Cancelled);
        assert_eq!(running.progress, 60);
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
}
