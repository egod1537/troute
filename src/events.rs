use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::api::OptimizeRouteResponse;

pub const MAX_JOB_ID_CHARACTERS: usize = 128;

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

/// Synchronous reporting boundary used by the optimization pipeline.
///
/// Implementations must return promptly. The HTTP application uses this
/// boundary to persist local job progress and terminal state.
pub trait OptimizationEventReporter: Send + Sync {
    fn progress(&self, stage: ProgressStage, progress: u8, message: Option<&str>);

    fn error(&self, code: OptimizationErrorCode, message: &str, detail: &str);

    fn result(&self, response: &OptimizeRouteResponse);
}

#[derive(Debug, Default)]
pub struct NoopOptimizationEventReporter;

impl OptimizationEventReporter for NoopOptimizationEventReporter {
    fn progress(&self, _stage: ProgressStage, _progress: u8, _message: Option<&str>) {}

    fn error(&self, _code: OptimizationErrorCode, _message: &str, _detail: &str) {}

    fn result(&self, _response: &OptimizeRouteResponse) {}
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
