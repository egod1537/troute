use super::{OptimizationServiceError, ProgressCoordinator, RouteOptimizationService};
use crate::{
    cancellation::CancellationToken,
    events::NoopOptimizationEventReporter,
    routing::RoutingProvider,
    solver::RouteSolver,
    validation::{validate_input, validate_schedule, ValidateRouteRequest, ValidateRouteResponse},
};
impl<P: RoutingProvider, S: RouteSolver> RouteOptimizationService<P, S> {
    pub fn validate_route(
        &self,
        mut request: ValidateRouteRequest,
        cancellation: &CancellationToken,
    ) -> Result<ValidateRouteResponse, OptimizationServiceError> {
        self.apply_request_defaults(&mut request.route);
        let problem = match validate_input(&request) {
            Ok(problem) => problem,
            Err(response) => return Ok(response),
        };
        let source = if request.route.use_euclidean_matrix {
            "euclidean"
        } else if request.route.travel_time_matrix.is_some()
            || request.route.sparse_travel_time_matrix.is_some()
        {
            "supplied"
        } else {
            "provider"
        };
        let progress = ProgressCoordinator::new(&NoopOptimizationEventReporter);
        let mut prepared = Self::prepare_request(request.route.clone(), &progress, cancellation)?;
        let routing = self.resolve_routing(&prepared, &progress, cancellation)?;
        let matrix = self.build_matrix(&mut prepared, &routing, &progress, cancellation)?;
        Ok(validate_schedule(&request, &problem, &matrix, source))
    }
}
