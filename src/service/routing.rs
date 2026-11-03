use crate::{
    api::OptimizeRouteRequest,
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    events::ProgressStage,
    matrix::{MatrixSource, TravelTimeMatrix},
    routing::{
        NoopRoutingProgressObserver, RouteProviderSelection, RoutingContext, RoutingError,
        RoutingProgressObserver, RoutingProvider,
    },
};

use super::{
    check_cancelled,
    preparation::{routing_departure_time, PreparedOptimization},
    OptimizationServiceError, ProgressCoordinator, RouteOptimizationService,
};

pub(super) struct ResolvedRouting {
    pub(super) context: Option<RoutingContext>,
    pub(super) selection: Option<RouteProviderSelection>,
}

struct RoutingProgressReporter<'a> {
    progress: &'a ProgressCoordinator<'a>,
    provider: Option<String>,
}

impl RoutingProgressObserver for RoutingProgressReporter<'_> {
    fn pairs_completed(&self, completed: usize, total: usize) {
        let range = ProgressStage::FetchingTravelTimes.range();
        let percentage = range.interpolate(completed, total);
        let message = match &self.provider {
            Some(provider) => {
                format!("Fetched {completed} / {total} travel-time pairs from {provider}.")
            }
            None => format!("Fetched {completed} / {total} travel-time pairs."),
        };
        self.progress
            .report(ProgressStage::FetchingTravelTimes, percentage, message);
    }
}

impl<P, S> RouteOptimizationService<P, S>
where
    P: RoutingProvider,
{
    pub(super) fn build_request_matrix(
        &self,
        request: OptimizeRouteRequest,
    ) -> Result<TravelTimeMatrix, OptimizationServiceError> {
        let travel_mode = request.travel_mode.unwrap_or_default();
        let requested_country = request.country_code.clone();
        let requested_provider = request.route_provider;
        let problem = OptimizationProblem::try_from(request.clone())?;
        if request.use_euclidean_matrix {
            return Ok(crate::euclidean::travel_time_matrix(
                &request.locations,
                travel_mode,
            ));
        }
        let (departure_time, time_zone) = routing_departure_time(&request)?;
        let routing_context = self.matrix_service.routing_context(
            departure_time,
            travel_mode,
            time_zone,
            requested_country.as_deref(),
            requested_provider,
        );
        self.matrix_service
            .build(
                problem.locations(),
                MatrixSource::Provider,
                &routing_context,
                &NoopRoutingProgressObserver,
                &CancellationToken::new(),
                None,
            )
            .map_err(Into::into)
    }

    pub(super) fn resolve_routing(
        &self,
        prepared: &PreparedOptimization,
        progress: &ProgressCoordinator<'_>,
        cancellation: &CancellationToken,
    ) -> Result<ResolvedRouting, OptimizationServiceError> {
        let has_local_matrix = prepared.has_local_matrix();
        progress.report(
            ProgressStage::SelectingProvider,
            ProgressStage::SelectingProvider.range().start,
            if prepared.euclidean_matrix.is_some() {
                "Using coordinates to estimate a straight-line matrix; no route provider is required."
            } else if has_local_matrix {
                "Using the caller-supplied matrix; no route provider is required."
            } else {
                "Selecting a route provider for the requested travel mode."
            },
        );
        check_cancelled(cancellation)?;
        if has_local_matrix {
            return Ok(ResolvedRouting {
                context: None,
                selection: None,
            });
        }

        let context = self.matrix_service.routing_context(
            prepared.matrix_departure_time,
            prepared.travel_mode,
            prepared.matrix_time_zone.clone(),
            prepared.country_code.as_deref(),
            prepared.requested_provider,
        );
        let selection = self.matrix_service.provider_selection(&context)?;
        if let Some(selection) = &selection {
            progress.report(
                ProgressStage::SelectingProvider,
                ProgressStage::SelectingProvider.range().start,
                format!("Selected {} as the route provider.", selection.provider),
            );
        }
        Ok(ResolvedRouting {
            context: Some(context),
            selection,
        })
    }

    pub(super) fn build_matrix(
        &self,
        prepared: &mut PreparedOptimization,
        routing: &ResolvedRouting,
        progress: &ProgressCoordinator<'_>,
        cancellation: &CancellationToken,
    ) -> Result<TravelTimeMatrix, OptimizationServiceError> {
        let pair_count = prepared
            .problem
            .locations()
            .len()
            .saturating_mul(prepared.problem.locations().len().saturating_sub(1));
        progress.report(
            ProgressStage::PreparingMatrix,
            ProgressStage::PreparingMatrix.range().start,
            format!("Prepared {pair_count} directed travel-time pairs."),
        );
        check_cancelled(cancellation)?;
        progress.report(
            ProgressStage::FetchingTravelTimes,
            ProgressStage::FetchingTravelTimes.range().start,
            if prepared.euclidean_matrix.is_some() {
                "Estimating travel times from straight-line coordinate distances.".to_owned()
            } else if prepared.has_local_matrix() {
                "Loading the caller-supplied travel-time matrix.".to_owned()
            } else {
                format!("Fetching travel times for {pair_count} directed pairs.")
            },
        );

        let source = match (
            prepared.supplied_matrix.take(),
            prepared.supplied_sparse_matrix.take(),
            prepared.euclidean_matrix.take(),
        ) {
            (Some(rows), None, None) | (None, None, Some(rows)) => MatrixSource::Dense(rows),
            (None, Some(rows), None) => MatrixSource::Sparse(rows),
            (None, None, None) => MatrixSource::Provider,
            _ => unreachable!("request validation rejects multiple matrix sources"),
        };
        let default_context = RoutingContext::default();
        let context = routing.context.as_ref().unwrap_or(&default_context);
        let routing_progress = RoutingProgressReporter {
            progress,
            provider: routing
                .selection
                .as_ref()
                .map(|selection| selection.provider.to_string()),
        };
        let matrix = match self.matrix_service.build(
            prepared.problem.locations(),
            source,
            context,
            &routing_progress,
            cancellation,
            None,
        ) {
            Ok(matrix) => matrix,
            Err(RoutingError::Cancelled) => return Err(OptimizationServiceError::Cancelled),
            Err(error) => return Err(error.into()),
        };
        check_cancelled(cancellation)?;
        progress.report(
            ProgressStage::BuildingMatrix,
            ProgressStage::BuildingMatrix.range().start,
            "Normalizing and validating the travel-time matrix.",
        );
        progress.report(
            ProgressStage::BuildingMatrix,
            ProgressStage::BuildingMatrix.range().end,
            "Travel-time matrix validation completed.",
        );
        Ok(matrix)
    }
}
