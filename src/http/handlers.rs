mod diagnostics;
mod events;
mod health;
mod jobs;
mod matrix;
mod optimization;

pub(super) use diagnostics::{provider_policy, solver_capabilities};
pub(super) use events::{job_events, solver_events};
pub(super) use health::health;
pub(super) use jobs::{cancel_job, get_job, job_timeline, list_jobs, solver_telemetry, submit_job};
pub(super) use matrix::build_matrix;
pub(super) use optimization::optimize;

mod validation;
pub(super) use validation::validate_route;
