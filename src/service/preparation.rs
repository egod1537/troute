use chrono::Duration;

use crate::{
    api::{requested_routing_departure, OptimizeRouteRequest, RequestValidationError},
    cancellation::CancellationToken,
    domain::{OptimizationProblem, TimeOfDay},
    events::ProgressStage,
    routing::{normalize_country_code, RouteProviderName, TravelMode},
    solver::PortfolioExperimentOptions,
};

use super::{
    check_cancelled, OptimizationServiceError, ProgressCoordinator, RouteOptimizationService,
};

const ROUTING_DEPARTURE_LEAD_MINUTES: i64 = 5;

pub(super) struct PreparedOptimization {
    pub(super) problem: OptimizationProblem,
    pub(super) supplied_matrix: Option<Vec<Vec<u32>>>,
    pub(super) supplied_sparse_matrix: Option<Vec<Vec<Option<u32>>>>,
    pub(super) shuffle_seed: Option<Option<u64>>,
    pub(super) portfolio_experiment: Option<PortfolioExperimentOptions>,
    pub(super) telemetry_job_id: String,
    pub(super) dated_routing: bool,
    pub(super) travel_mode: TravelMode,
    pub(super) country_code: Option<String>,
    pub(super) requested_provider: Option<RouteProviderName>,
    pub(super) matrix_departure_time: chrono::DateTime<chrono::Utc>,
    pub(super) matrix_time_zone: Option<String>,
}

impl PreparedOptimization {
    pub(super) fn has_supplied_matrix(&self) -> bool {
        self.supplied_matrix.is_some() || self.supplied_sparse_matrix.is_some()
    }
}

impl<P, S> RouteOptimizationService<P, S> {
    pub(super) fn prepare_request(
        request: OptimizeRouteRequest,
        progress: &ProgressCoordinator<'_>,
        cancellation: &CancellationToken,
    ) -> Result<PreparedOptimization, OptimizationServiceError> {
        progress.report(
            ProgressStage::Accepted,
            ProgressStage::Accepted.range().start,
            "Optimization request accepted.",
        );
        check_cancelled(cancellation)?;
        progress.report(
            ProgressStage::ValidatingRequest,
            ProgressStage::ValidatingRequest.range().start,
            "Validating locations and schedule constraints.",
        );
        let shuffle_seed = request.debug.as_ref().and_then(|debug| {
            debug
                .shuffle_result_route
                .unwrap_or(false)
                .then_some(debug.shuffle_seed)
        });
        let portfolio_experiment = request.portfolio_experiment.clone();
        let telemetry_job_id = request.job_id.clone();
        let supplied_matrix = request.travel_time_matrix.clone();
        let supplied_sparse_matrix = request.sparse_travel_time_matrix.clone();
        let dated_routing = request.travel_date.is_some();
        let travel_mode = request.travel_mode.unwrap_or_default();
        let requested_country = request.country_code.clone();
        let requested_provider = request.route_provider;
        let (matrix_departure_time, matrix_time_zone) = routing_departure_time(&request)?;
        let problem = OptimizationProblem::try_from(request)?;
        let country_code = requested_country
            .as_deref()
            .map(normalize_country_code)
            .transpose()
            .expect("request validation normalizes country codes");
        check_cancelled(cancellation)?;

        Ok(PreparedOptimization {
            problem,
            supplied_matrix,
            supplied_sparse_matrix,
            shuffle_seed,
            portfolio_experiment,
            telemetry_job_id,
            dated_routing,
            travel_mode,
            country_code,
            requested_provider,
            matrix_departure_time,
            matrix_time_zone,
        })
    }
}

pub(super) fn routing_departure_time(
    request: &OptimizeRouteRequest,
) -> Result<(chrono::DateTime<chrono::Utc>, Option<String>), RequestValidationError> {
    let start_time = request
        .start_time
        .unwrap_or_else(|| TimeOfDay::from_minutes(0).expect("midnight is valid"));
    Ok(requested_routing_departure(request, start_time)?
        .map(|(departure, time_zone)| (departure, Some(time_zone)))
        .unwrap_or_else(|| {
            (
                chrono::Utc::now() + Duration::minutes(ROUTING_DEPARTURE_LEAD_MINUTES),
                None,
            )
        }))
}
