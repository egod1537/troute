use thiserror::Error;

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse, RequestValidationError},
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    events::{
        NoopOptimizationEventReporter, OptimizationErrorCode, OptimizationEventReporter,
        ProgressStage,
    },
    result_debug::shuffle_solution,
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
        self.optimize_with_reporter(request, &NoopOptimizationEventReporter)
    }

    pub fn optimize_with_reporter(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        self.optimize_with_cancellation(request, reporter, &CancellationToken::new())
    }

    pub fn optimize_with_cancellation(
        &self,
        request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        let result = (|| {
            let shuffle_options = request.debug.as_ref().and_then(|debug| {
                debug
                    .shuffle_result_route
                    .unwrap_or(false)
                    .then_some(debug.shuffle_seed)
            });
            let problem = OptimizationProblem::try_from(request)?;
            reporter.progress(
                ProgressStage::Accepted,
                0,
                Some("Optimization request accepted."),
            );
            check_cancelled(cancellation)?;
            reporter.progress(
                ProgressStage::BuildingMatrix,
                20,
                Some("Building travel-time matrix."),
            );
            check_cancelled(cancellation)?;
            let matrix = self
                .routing_provider
                .travel_time_matrix(problem.locations())?;
            check_cancelled(cancellation)?;
            reporter.progress(ProgressStage::Solving, 60, Some("Optimizing visit order."));
            check_cancelled(cancellation)?;
            let solution = match self.solver.solve(SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation,
            }) {
                Ok(solution) => solution,
                Err(SolverError::Cancelled) => return Err(OptimizationServiceError::Cancelled),
                Err(error) => return Err(error.into()),
            };
            check_cancelled(cancellation)?;
            reporter.progress(
                ProgressStage::Scheduling,
                85,
                Some("Building itinerary schedule."),
            );
            check_cancelled(cancellation)?;
            let normal_plan = calculate_schedule(&problem, &matrix, &solution)?;
            check_cancelled(cancellation)?;
            let plan = match shuffle_options {
                Some(seed) => {
                    let shuffled_solution = shuffle_solution(&solution, seed);
                    calculate_schedule(&problem, &matrix, &shuffled_solution)?
                }
                None => normal_plan,
            };
            check_cancelled(cancellation)?;
            let response = OptimizeRouteResponse::from_plan(plan, &problem);
            check_cancelled(cancellation)?;
            reporter.result(&response);
            check_cancelled(cancellation)?;

            Ok(response)
        })();

        if let Err(error) = &result {
            report_error(reporter, error);
        }

        if cancellation.is_cancelled() {
            Err(OptimizationServiceError::Cancelled)
        } else {
            result
        }
    }
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), OptimizationServiceError> {
    if cancellation.is_cancelled() {
        Err(OptimizationServiceError::Cancelled)
    } else {
        Ok(())
    }
}

fn report_error(reporter: &dyn OptimizationEventReporter, error: &OptimizationServiceError) {
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{
        development::{DevelopmentRouteSolver, DevelopmentRoutingProvider},
        domain::Location,
        events::{OptimizationErrorCode, ProgressStage},
        matrix::TravelTimeMatrix,
        solver::SolverSolution,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum RecordedEvent {
        Progress(ProgressStage, u8),
        Error(OptimizationErrorCode),
        Result(OptimizeRouteResponse),
    }

    #[derive(Default)]
    struct InMemoryReporter {
        events: Mutex<Vec<RecordedEvent>>,
    }

    impl InMemoryReporter {
        fn events(&self) -> Vec<RecordedEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    impl OptimizationEventReporter for InMemoryReporter {
        fn progress(&self, stage: ProgressStage, progress: u8, _message: Option<&str>) {
            self.events
                .lock()
                .unwrap()
                .push(RecordedEvent::Progress(stage, progress));
        }

        fn error(&self, code: OptimizationErrorCode, _message: &str, _detail: &str) {
            self.events.lock().unwrap().push(RecordedEvent::Error(code));
        }

        fn result(&self, response: &OptimizeRouteResponse) {
            self.events
                .lock()
                .unwrap()
                .push(RecordedEvent::Result(response.clone()));
        }
    }

    struct FailingRoutingProvider;

    impl RoutingProvider for FailingRoutingProvider {
        fn travel_time_matrix(
            &self,
            _locations: &[Location],
        ) -> Result<TravelTimeMatrix, RoutingError> {
            Err(RoutingError::Provider("provider is offline".to_owned()))
        }
    }

    struct UnusedSolver;

    impl RouteSolver for UnusedSolver {
        fn solve(&self, _input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
            unreachable!("routing failure must stop before solving")
        }
    }

    struct FixedRoutingProvider;

    impl RoutingProvider for FixedRoutingProvider {
        fn travel_time_matrix(
            &self,
            _locations: &[Location],
        ) -> Result<TravelTimeMatrix, RoutingError> {
            TravelTimeMatrix::new(vec![vec![0]])
                .map_err(|error| RoutingError::Provider(error.to_string()))
        }
    }

    struct NoFeasibleSolver;

    impl RouteSolver for NoFeasibleSolver {
        fn solve(&self, _input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
            Err(SolverError::NoFeasibleRoute)
        }
    }

    #[test]
    fn success_reports_real_pipeline_boundaries_in_monotonic_order() {
        let service =
            RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver);
        let reporter = InMemoryReporter::default();

        let response = service
            .optimize_with_reporter(valid_request(), &reporter)
            .unwrap();

        assert_eq!(
            reporter.events(),
            vec![
                RecordedEvent::Progress(ProgressStage::Accepted, 0),
                RecordedEvent::Progress(ProgressStage::BuildingMatrix, 20),
                RecordedEvent::Progress(ProgressStage::Solving, 60),
                RecordedEvent::Progress(ProgressStage::Scheduling, 85),
                RecordedEvent::Result(response),
            ]
        );
    }

    #[test]
    fn cancelled_pipeline_stops_before_matrix_and_emits_no_error_or_result() {
        let service =
            RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver);
        let reporter = InMemoryReporter::default();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = service
            .optimize_with_cancellation(valid_request(), &reporter, &cancellation)
            .unwrap_err();

        assert!(matches!(error, OptimizationServiceError::Cancelled));
        assert_eq!(
            reporter.events(),
            vec![RecordedEvent::Progress(ProgressStage::Accepted, 0)]
        );
    }

    #[test]
    fn routing_failure_reports_a_stable_error() {
        let service = RouteOptimizationService::new(FailingRoutingProvider, UnusedSolver);
        let reporter = InMemoryReporter::default();

        assert!(service
            .optimize_with_reporter(valid_request(), &reporter)
            .is_err());

        assert_eq!(
            reporter.events(),
            vec![
                RecordedEvent::Progress(ProgressStage::Accepted, 0),
                RecordedEvent::Progress(ProgressStage::BuildingMatrix, 20),
                RecordedEvent::Error(OptimizationErrorCode::RoutingUnavailable),
            ]
        );
    }

    #[test]
    fn no_feasible_route_reports_the_contract_error_code() {
        let service = RouteOptimizationService::new(FixedRoutingProvider, NoFeasibleSolver);
        let reporter = InMemoryReporter::default();

        assert!(service
            .optimize_with_reporter(valid_request(), &reporter)
            .is_err());

        assert_eq!(
            reporter.events(),
            vec![
                RecordedEvent::Progress(ProgressStage::Accepted, 0),
                RecordedEvent::Progress(ProgressStage::BuildingMatrix, 20),
                RecordedEvent::Progress(ProgressStage::Solving, 60),
                RecordedEvent::Error(OptimizationErrorCode::NoFeasibleRoute),
            ]
        );
    }

    fn valid_request() -> OptimizeRouteRequest {
        serde_json::from_value(serde_json::json!({
            "job_id": "route-service-test",
            "locations": [
                {
                    "id": "A",
                    "place_id": "place-a",
                    "open_time": "00:00",
                    "close_time": "23:59",
                    "stay_minutes": 0
                },
                {
                    "id": "B",
                    "place_id": "place-b",
                    "open_time": "00:00",
                    "close_time": "23:59",
                    "stay_minutes": 0
                }
            ],
            "start_time": "09:00"
        }))
        .unwrap()
    }
}
