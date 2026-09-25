use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::api::OptimizeRouteResponse;

pub const MAX_JOB_ID_CHARACTERS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStage {
    Accepted,
    ValidatingRequest,
    SelectingProvider,
    PreparingMatrix,
    FetchingTravelTimes,
    BuildingMatrix,
    GeneratingCandidates,
    #[serde(alias = "solving")]
    OptimizingRoute,
    SelectingBestCandidate,
    Scheduling,
    ValidatingSchedule,
    FinalizingResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressRange {
    pub start: u8,
    pub end: u8,
}

impl ProgressRange {
    pub const fn point(value: u8) -> Self {
        Self {
            start: value,
            end: value,
        }
    }

    pub fn interpolate(self, completed: usize, total: usize) -> u8 {
        if total == 0 {
            return self.start;
        }
        let width = usize::from(self.end.saturating_sub(self.start));
        self.start.saturating_add(
            u8::try_from(width.saturating_mul(completed.min(total)) / total).unwrap_or(width as u8),
        )
    }
}

impl ProgressStage {
    pub const fn range(self) -> ProgressRange {
        match self {
            Self::Accepted => ProgressRange::point(0),
            Self::ValidatingRequest => ProgressRange::point(5),
            Self::SelectingProvider => ProgressRange::point(10),
            Self::PreparingMatrix => ProgressRange::point(15),
            Self::FetchingTravelTimes => ProgressRange { start: 20, end: 45 },
            Self::BuildingMatrix => ProgressRange { start: 48, end: 52 },
            Self::GeneratingCandidates => ProgressRange { start: 55, end: 65 },
            Self::OptimizingRoute => ProgressRange { start: 65, end: 80 },
            Self::SelectingBestCandidate => ProgressRange { start: 82, end: 86 },
            Self::Scheduling => ProgressRange { start: 88, end: 92 },
            Self::ValidatingSchedule => ProgressRange { start: 94, end: 96 },
            Self::FinalizingResult => ProgressRange::point(98),
        }
    }
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
        let stages = [
            (ProgressStage::Accepted, "accepted"),
            (ProgressStage::ValidatingRequest, "validating_request"),
            (ProgressStage::SelectingProvider, "selecting_provider"),
            (ProgressStage::PreparingMatrix, "preparing_matrix"),
            (ProgressStage::FetchingTravelTimes, "fetching_travel_times"),
            (ProgressStage::BuildingMatrix, "building_matrix"),
            (ProgressStage::GeneratingCandidates, "generating_candidates"),
            (ProgressStage::OptimizingRoute, "optimizing_route"),
            (
                ProgressStage::SelectingBestCandidate,
                "selecting_best_candidate",
            ),
            (ProgressStage::Scheduling, "scheduling"),
            (ProgressStage::ValidatingSchedule, "validating_schedule"),
            (ProgressStage::FinalizingResult, "finalizing_result"),
        ];
        for (stage, wire) in stages {
            assert_eq!(serde_json::to_value(stage).unwrap(), wire);
        }
        assert_eq!(
            serde_json::from_str::<ProgressStage>(r#""solving""#).unwrap(),
            ProgressStage::OptimizingRoute
        );
        assert_eq!(
            serde_json::to_value(OptimizationErrorCode::NoFeasibleRoute).unwrap(),
            "NO_FEASIBLE_ROUTE"
        );
    }

    #[test]
    fn progress_ranges_are_ordered_and_bounded() {
        let stages = [
            ProgressStage::Accepted,
            ProgressStage::ValidatingRequest,
            ProgressStage::SelectingProvider,
            ProgressStage::PreparingMatrix,
            ProgressStage::FetchingTravelTimes,
            ProgressStage::BuildingMatrix,
            ProgressStage::GeneratingCandidates,
            ProgressStage::OptimizingRoute,
            ProgressStage::SelectingBestCandidate,
            ProgressStage::Scheduling,
            ProgressStage::ValidatingSchedule,
            ProgressStage::FinalizingResult,
        ];
        assert!(stages.windows(2).all(|pair| {
            pair[0].range().start <= pair[0].range().end
                && pair[0].range().end <= pair[1].range().start
        }));
        assert!(stages.last().unwrap().range().end < 100);
    }
}
