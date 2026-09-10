use thiserror::Error;

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse, RequestValidationError},
    domain::OptimizationProblem,
    routing::{RoutingError, RoutingProvider},
    schedule::{calculate_schedule, ScheduleError},
    solver::{RouteSolver, SolverError, SolverInput},
};

/// Coordinates the provider -> solver -> schedule pipeline without coupling
/// any of those components to an HTTP framework.
pub struct RouteOptimizationService<P, S> {
    routing_provider: P,
    solver: S,
}

impl<P, S> RouteOptimizationService<P, S>
where
    P: RoutingProvider,
    S: RouteSolver,
{
    pub fn new(routing_provider: P, solver: S) -> Self {
        Self {
            routing_provider,
            solver,
        }
    }

    pub fn optimize(
        &self,
        request: OptimizeRouteRequest,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        let problem = OptimizationProblem::try_from(request)?;
        let matrix = self
            .routing_provider
            .travel_time_matrix(problem.locations())?;
        let solution = self.solver.solve(SolverInput {
            matrix: &matrix,
            problem: &problem,
        })?;
        let plan = calculate_schedule(&problem, &matrix, &solution)?;

        Ok(OptimizeRouteResponse::from_plan(plan, &problem))
    }
}

#[derive(Debug, Error)]
pub enum OptimizationServiceError {
    #[error(transparent)]
    InvalidRequest(#[from] RequestValidationError),
    #[error(transparent)]
    Routing(#[from] RoutingError),
    #[error(transparent)]
    Solver(#[from] SolverError),
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
}
