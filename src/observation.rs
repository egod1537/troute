use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use axum::http::{header, HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::logging::logger;

pub const DEFAULT_MAX_OBSERVED_JOBS: usize = 100;
pub const DEFAULT_MAX_ENTRIES_PER_JOB: usize = 500;

static NEXT_ENTRY_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_PAIR_ID: AtomicU64 = AtomicU64::new(1);

pub type ObservationHeaders = BTreeMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ObservationPeer {
    Testbed,
    Troute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObservationDirection {
    Request,
    Response,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JobTimelineEntry {
    pub id: String,
    pub pair_id: String,
    pub timestamp_ms: i64,
    pub direction: ObservationDirection,
    pub source: ObservationPeer,
    pub target: ObservationPeer,
    pub method: Option<String>,
    pub path: Option<String>,
    pub status: Option<u16>,
    pub latency_ms: Option<f64>,
    pub headers: Option<ObservationHeaders>,
    pub query: Option<Value>,
    pub body: Option<Value>,
    pub raw: Option<String>,
    pub error: Option<String>,
}

impl JobTimelineEntry {
    fn new(
        pair_id: String,
        direction: ObservationDirection,
        source: ObservationPeer,
        target: ObservationPeer,
    ) -> Self {
        Self {
            id: format!("evt-{:06}", NEXT_ENTRY_ID.fetch_add(1, Ordering::Relaxed)),
            pair_id,
            timestamp_ms: timestamp_ms(),
            direction,
            source,
            target,
            method: None,
            path: None,
            status: None,
            latency_ms: None,
            headers: None,
            query: None,
            body: None,
            raw: None,
            error: None,
        }
    }
}

pub trait JobTimelineStore: Send + Sync {
    fn append(&self, job_id: &str, entry: JobTimelineEntry);
    fn list(&self, job_id: &str) -> Vec<JobTimelineEntry>;
    fn clear(&self, job_id: &str);
}

#[derive(Debug)]
pub struct InMemoryJobTimelineStore {
    max_jobs: usize,
    max_entries_per_job: usize,
    state: Mutex<StoreState>,
}

#[derive(Debug, Default)]
struct StoreState {
    jobs: HashMap<String, VecDeque<JobTimelineEntry>>,
    job_order: VecDeque<String>,
}

impl InMemoryJobTimelineStore {
    pub fn new(max_jobs: usize, max_entries_per_job: usize) -> Self {
        Self {
            max_jobs: max_jobs.max(1),
            max_entries_per_job: max_entries_per_job.max(1),
            state: Mutex::new(StoreState::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, StoreState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for InMemoryJobTimelineStore {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_OBSERVED_JOBS, DEFAULT_MAX_ENTRIES_PER_JOB)
    }
}

impl JobTimelineStore for InMemoryJobTimelineStore {
    fn append(&self, job_id: &str, entry: JobTimelineEntry) {
        let mut state = self.state();
        if !state.jobs.contains_key(job_id) {
            while state.jobs.len() >= self.max_jobs {
                let Some(oldest_job_id) = state.job_order.pop_front() else {
                    break;
                };
                state.jobs.remove(&oldest_job_id);
            }
            state.job_order.push_back(job_id.to_owned());
            state.jobs.insert(job_id.to_owned(), VecDeque::new());
        }

        if let Some(entries) = state.jobs.get_mut(job_id) {
            entries.push_back(entry);
            while entries.len() > self.max_entries_per_job {
                entries.pop_front();
            }
        }
    }

    fn list(&self, job_id: &str) -> Vec<JobTimelineEntry> {
        self.state()
            .jobs
            .get(job_id)
            .map(|entries| entries.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn clear(&self, job_id: &str) {
        let mut state = self.state();
        state.jobs.remove(job_id);
        state
            .job_order
            .retain(|stored_job_id| stored_job_id != job_id);
    }
}

#[derive(Clone)]
pub struct JobObservationRecorder {
    store: Arc<dyn JobTimelineStore>,
}

impl JobObservationRecorder {
    pub fn new(store: Arc<dyn JobTimelineStore>) -> Self {
        Self { store }
    }

    pub fn in_memory() -> Self {
        Self::new(Arc::new(InMemoryJobTimelineStore::default()))
    }

    pub fn next_pair_id(&self, prefix: &str) -> String {
        format!(
            "{prefix}-{:06}",
            NEXT_PAIR_ID.fetch_add(1, Ordering::Relaxed)
        )
    }

    pub fn entry(
        &self,
        pair_id: impl Into<String>,
        direction: ObservationDirection,
        source: ObservationPeer,
        target: ObservationPeer,
    ) -> JobTimelineEntry {
        JobTimelineEntry::new(pair_id.into(), direction, source, target)
    }

    pub fn append(&self, job_id: &str, entry: JobTimelineEntry) {
        if catch_unwind(AssertUnwindSafe(|| self.store.append(job_id, entry))).is_err() {
            logger().job_operation_panicked("job observation append", job_id);
        }
    }

    pub fn list(&self, job_id: &str) -> Vec<JobTimelineEntry> {
        catch_unwind(AssertUnwindSafe(|| self.store.list(job_id))).unwrap_or_else(|_| {
            logger().job_operation_panicked("job observation read", job_id);
            Vec::new()
        })
    }

    pub fn clear(&self, job_id: &str) {
        if catch_unwind(AssertUnwindSafe(|| self.store.clear(job_id))).is_err() {
            logger().job_operation_panicked("job observation clear", job_id);
        }
    }
}

pub fn allowlisted_headers(headers: &HeaderMap) -> Option<ObservationHeaders> {
    let mut allowed = ObservationHeaders::new();
    for name in [header::CONTENT_TYPE, header::ACCEPT, header::USER_AGENT] {
        let values = headers
            .get_all(&name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>();
        if !values.is_empty() {
            allowed.insert(name.as_str().to_owned(), values.join(", "));
        }
    }
    (!allowed.is_empty()).then_some(allowed)
}

pub fn json_or_raw(raw: String) -> (Option<Value>, Option<String>) {
    if raw.is_empty() {
        return (None, None);
    }
    match serde_json::from_str(&raw) {
        Ok(json) => (Some(json), None),
        Err(_) => (None, Some(raw)),
    }
}

fn timestamp_ms() -> i64 {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(milliseconds).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests;
