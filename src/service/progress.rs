use std::sync::Mutex;

use crate::{
    events::{OptimizationErrorCode, OptimizationEventReporter, ProgressStage},
    schedule::ScheduleError,
    solver::SolverError,
};

use super::OptimizationServiceError;

struct ProgressState {
    stage: Option<ProgressStage>,
    progress: u8,
}

pub(super) struct ProgressCoordinator<'a> {
    reporter: &'a dyn OptimizationEventReporter,
    state: Mutex<ProgressState>,
}

impl<'a> ProgressCoordinator<'a> {
    pub(super) fn new(reporter: &'a dyn OptimizationEventReporter) -> Self {
        Self {
            reporter,
            state: Mutex::new(ProgressState {
                stage: None,
                progress: 0,
            }),
        }
    }

    pub(super) fn report(&self, stage: ProgressStage, progress: u8, message: impl AsRef<str>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stage.is_some_and(|current| stage < current) {
            return;
        }
        let progress = state.progress.max(progress.min(100));
        if state.stage == Some(stage) && state.progress == progress {
            return;
        }
        state.stage = Some(stage);
        state.progress = progress;
        self.reporter
            .progress(stage, progress, Some(message.as_ref()));
    }
}

pub(super) fn report_error(
    reporter: &dyn OptimizationEventReporter,
    error: &OptimizationServiceError,
) {
    match error {
        OptimizationServiceError::Cancelled => {}
        OptimizationServiceError::InvalidRequest(source) => reporter.error(
            OptimizationErrorCode::InvalidRequest,
            "The optimize request is invalid.",
            &source.to_string(),
        ),
        OptimizationServiceError::Routing(source) => reporter.error(
            OptimizationErrorCode::RoutingUnavailable,
            "Travel-time routing is temporarily unavailable.",
            &source.to_string(),
        ),
        OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => reporter.error(
            OptimizationErrorCode::NoFeasibleRoute,
            "No feasible route was found.",
            "solver found no feasible route",
        ),
        OptimizationServiceError::Schedule(
            source @ (ScheduleError::OutsideSingleDay | ScheduleError::TimeWindowViolation { .. }),
        ) => reporter.error(
            OptimizationErrorCode::NoFeasibleRoute,
            "No feasible route was found.",
            &source.to_string(),
        ),
        OptimizationServiceError::Solver(source) => reporter.error(
            OptimizationErrorCode::SolverError,
            "Route optimization failed unexpectedly.",
            &source.to_string(),
        ),
        OptimizationServiceError::Schedule(source) => reporter.error(
            OptimizationErrorCode::ScheduleError,
            "Route scheduling failed unexpectedly.",
            &source.to_string(),
        ),
    }
}
