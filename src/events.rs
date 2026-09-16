use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::api::OptimizeRouteResponse;

pub const MAX_JOB_ID_CHARACTERS: usize = 128;
pub const FIRST_JOB_EVENT_SEQUENCE: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JobEventType {
    Progress,
    Error,
    Result,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStage {
    Accepted,
    BuildingMatrix,
    Solving,
    Scheduling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum OptimizationErrorCode {
    InvalidRequest,
    RoutingUnavailable,
    NoFeasibleRoute,
    SolverError,
    ScheduleError,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProgressEventData {
    pub stage: ProgressStage,
    pub progress: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ErrorEventData {
    pub code: OptimizationErrorCode,
    pub message: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct CancelledEventData {
    pub message: String,
}

/// Synchronous reporting boundary used by the optimization pipeline.
///
/// Implementations must return promptly. The HTTP application uses a channel
/// implementation so callback I/O happens asynchronously outside the service.
pub trait OptimizationEventReporter: Send + Sync {
    fn progress(&self, stage: ProgressStage, progress: u8, message: Option<&str>);

    fn error(&self, code: OptimizationErrorCode, message: &str, detail: &str);

    fn result(&self, response: &OptimizeRouteResponse);

    fn cancelled(&self, _message: &str) {}
}

#[derive(Debug, Default)]
pub struct NoopOptimizationEventReporter;

impl OptimizationEventReporter for NoopOptimizationEventReporter {
    fn progress(&self, _stage: ProgressStage, _progress: u8, _message: Option<&str>) {}

    fn error(&self, _code: OptimizationErrorCode, _message: &str, _detail: &str) {}

    fn result(&self, _response: &OptimizeRouteResponse) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct JobEvent<T = Value> {
    pub sequence: u32,
    #[serde(rename = "type")]
    pub event_type: JobEventType,
    pub data: T,
}

/// Owns the event sequence for one job and creates events in order.
#[derive(Debug)]
pub struct JobEventContext {
    job_id: String,
    next_sequence: Option<u32>,
}

impl JobEventContext {
    pub fn new(job_id: impl Into<String>) -> Result<Self, JobIdError> {
        let job_id = job_id.into();
        validate_job_id(&job_id)?;
        Ok(Self {
            job_id,
            next_sequence: Some(FIRST_JOB_EVENT_SEQUENCE),
        })
    }

    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn next_event<T>(
        &mut self,
        event_type: JobEventType,
        data: T,
    ) -> Result<JobEvent<T>, JobEventSequenceError> {
        let sequence = self.next_sequence.ok_or(JobEventSequenceError::Exhausted)?;
        self.next_sequence = sequence.checked_add(1);
        Ok(JobEvent {
            sequence,
            event_type,
            data,
        })
    }
}

pub(crate) fn validate_job_id(job_id: &str) -> Result<(), JobIdError> {
    if job_id.trim().is_empty() {
        return Err(JobIdError::Empty);
    }
    let characters = job_id.chars().count();
    if characters > MAX_JOB_ID_CHARACTERS {
        return Err(JobIdError::TooLong {
            maximum: MAX_JOB_ID_CHARACTERS,
            actual: characters,
        });
    }
    Ok(())
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum JobIdError {
    #[error("job_id must not be empty or whitespace only")]
    Empty,
    #[error("job_id contains {actual} characters; at most {maximum} are allowed")]
    TooLong { maximum: usize, actual: usize },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum JobEventSequenceError {
    #[error("job event sequence is exhausted")]
    Exhausted,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn event_type_wire_values_are_exact() {
        for (event_type, expected) in [
            (JobEventType::Progress, "progress"),
            (JobEventType::Error, "error"),
            (JobEventType::Result, "result"),
            (JobEventType::Cancelled, "cancelled"),
        ] {
            assert_eq!(serde_json::to_value(event_type).unwrap(), expected);
        }
    }

    #[test]
    fn progress_stages_and_error_codes_have_stable_wire_values() {
        assert_eq!(
            serde_json::to_value(ProgressStage::BuildingMatrix).unwrap(),
            "building_matrix"
        );
        assert_eq!(
            serde_json::to_value(OptimizationErrorCode::NoFeasibleRoute).unwrap(),
            "NO_FEASIBLE_ROUTE"
        );
    }

    #[test]
    fn event_envelope_serializes_with_type_field() {
        let event = JobEvent {
            sequence: 1,
            event_type: JobEventType::Progress,
            data: json!({ "completed": 2 }),
        };

        assert_eq!(
            serde_json::to_value(event).unwrap(),
            json!({
                "sequence": 1,
                "type": "progress",
                "data": { "completed": 2 }
            })
        );
    }

    #[test]
    fn sequence_starts_at_one_and_is_scoped_to_each_job() {
        let mut first = JobEventContext::new("route-first").unwrap();
        let mut second = JobEventContext::new("route-second").unwrap();

        assert_eq!(
            first
                .next_event(JobEventType::Progress, json!({}))
                .unwrap()
                .sequence,
            1
        );
        assert_eq!(
            first
                .next_event(JobEventType::Result, json!({}))
                .unwrap()
                .sequence,
            2
        );
        assert_eq!(
            second
                .next_event(JobEventType::Progress, json!({}))
                .unwrap()
                .sequence,
            1
        );
    }
}
