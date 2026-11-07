mod error;
mod execution;
mod finalization;
mod preparation;
mod progress;
mod remediation;
mod revalidation;
mod routing;

use std::sync::Arc;

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse},
    cancellation::CancellationToken,
    events::{NoopOptimizationEventReporter, OptimizationEventReporter},
    failure::{FailureSuggestion, RemediationConfig},
    matrix::MatrixService,
    routing::{RouteProviderSelectionDiagnostics, RoutingProvider},
    solver::{PortfolioCapabilities, RouteSolver},
};

pub use error::OptimizationServiceError;

use progress::{report_error, ProgressCoordinator};

/// Coordinates the provider -> solver -> schedule pipeline without coupling
/// any of those components to an HTTP framework.
pub struct RouteOptimizationService<P, S> {
    matrix_service: Arc<MatrixService<P>>,
    solver: S,
    remediation: RemediationConfig,
    default_euclidean_matrix: bool,
}

impl<P, S> RouteOptimizationService<P, S>
where
    P: RoutingProvider,
    S: RouteSolver,
{
    pub fn new(routing_provider: P, solver: S) -> Self {
        Self::with_matrix_service(Arc::new(MatrixService::new(routing_provider)), solver)
    }

    pub fn with_matrix_service(matrix_service: Arc<MatrixService<P>>, solver: S) -> Self {
        Self {
            matrix_service,
            solver,
            remediation: RemediationConfig::default(),
            default_euclidean_matrix: false,
        }
    }

    pub fn with_remediation_config(mut self, remediation: RemediationConfig) -> Self {
        self.remediation = remediation;
        self
    }

    /// Uses the local coordinate estimate whenever a request does not supply a
    /// matrix. Disabling the process setting restores provider-backed routing.
    pub fn with_default_euclidean_matrix(mut self, enabled: bool) -> Self {
        self.default_euclidean_matrix = enabled;
        self
    }

    /// Applies process-level request defaults before validation and storage.
    pub fn apply_request_defaults(&self, request: &mut OptimizeRouteRequest) {
        if self.default_euclidean_matrix
            && request.travel_time_matrix.is_none()
            && request.sparse_travel_time_matrix.is_none()
        {
            request.use_euclidean_matrix = true;
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
        let progress = ProgressCoordinator::new(reporter);
        let result = self.optimize_pipeline(request, reporter, cancellation, &progress);

        if let Err(error) = &result {
            report_error(reporter, error);
        }

        if cancellation.is_cancelled() {
            Err(OptimizationServiceError::Cancelled)
        } else {
            result
        }
    }

    pub fn build_travel_time_matrix(
        &self,
        mut request: OptimizeRouteRequest,
    ) -> Result<crate::matrix::TravelTimeMatrix, OptimizationServiceError> {
        self.apply_request_defaults(&mut request);
        self.build_request_matrix(request)
    }

    pub fn provider_policy_diagnostics(&self) -> Option<RouteProviderSelectionDiagnostics> {
        self.matrix_service.provider_policy_diagnostics()
    }

    pub fn solver_capabilities(&self) -> Option<PortfolioCapabilities> {
        self.solver.portfolio_capabilities()
    }

    pub fn failure_suggestions(
        &self,
        request: &OptimizeRouteRequest,
        error: &OptimizationServiceError,
        cancellation: &CancellationToken,
    ) -> Vec<FailureSuggestion> {
        let mut request = request.clone();
        self.apply_request_defaults(&mut request);
        self.failure_suggestions_for(&request, error, cancellation)
    }

    fn optimize_pipeline(
        &self,
        mut request: OptimizeRouteRequest,
        reporter: &dyn OptimizationEventReporter,
        cancellation: &CancellationToken,
        progress: &ProgressCoordinator<'_>,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        self.apply_request_defaults(&mut request);
        let mut prepared = Self::prepare_request(request, progress, cancellation)?;
        let routing = self.resolve_routing(&prepared, progress, cancellation)?;
        let matrix = self.build_matrix(&mut prepared, &routing, progress, cancellation)?;
        let (solver_result, selected_start_time) =
            self.solve_prepared(&prepared, &matrix, reporter, progress, cancellation)?;

        self.finalize_response(
            finalization::FinalizationInput {
                prepared: &prepared,
                routing: &routing,
                matrix: &matrix,
                solver_result,
                selected_start_time,
            },
            reporter,
            progress,
            cancellation,
        )
    }
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), OptimizationServiceError> {
    if cancellation.is_cancelled() {
        Err(OptimizationServiceError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
use std::time::Instant;

#[cfg(test)]
use crate::{
    domain::TimeOfDay,
    routing::{RoutingContext, RoutingError},
    solver::{SolverError, SolverInput},
};

#[cfg(test)]
mod tests;
