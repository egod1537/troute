use thiserror::Error;

use crate::{
    api::RequestValidationError, routing::RoutingError, schedule::ScheduleError,
    solver::SolverError,
};

#[derive(Debug, Error)]
pub enum OptimizationServiceError {
    #[error("optimization job was cancelled")]
    Cancelled,
    #[error(transparent)]
    InvalidRequest(#[from] RequestValidationError),
    #[error(transparent)]
    Routing(#[from] RoutingError),
    #[error(transparent)]
    Solver(#[from] SolverError),
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
}
