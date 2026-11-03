use std::time::Instant;

use rand::{rngs::StdRng, SeedableRng};

use crate::{
    api::OptimizeRouteRequest,
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    failure::{
        change_travel_mode_suggestion, rank_failure_suggestions, remove_location_suggestion,
        service_suggestions, split_day_probe_suggestion, FailureSuggestion, RemediationConfig,
    },
    matrix::{MatrixSource, TravelTimeMatrix},
    routing::{RoutingError, RoutingProvider, TravelMode},
    schedule::ScheduleError,
    solver::{
        DefaultObjectivePolicy, ExactBitDpSolver, GreedyInitialRoute, InitialRouteGenerator,
        ObjectiveEvaluator, RouteSolver, SolverError, SolverInput, EXACT_MAX_LOCATIONS,
    },
};

use super::{
    preparation::routing_departure_time, OptimizationServiceError, RouteOptimizationService,
};

/// Owns the budget, cancellation view, probe cap, and deduplicated findings
/// shared by one bounded failure-remediation run.
struct RemediationRun<'a> {
    deadline: Instant,
    cancellation: &'a CancellationToken,
    max_remove_probes: usize,
    suggestions: Vec<FailureSuggestion>,
}

impl<'a> RemediationRun<'a> {
    fn new(
        config: RemediationConfig,
        cancellation: &'a CancellationToken,
        suggestions: Vec<FailureSuggestion>,
    ) -> Result<Self, Vec<FailureSuggestion>> {
        if cancellation.is_cancelled() || config.budget.is_zero() {
            return Err(suggestions);
        }
        let Some(deadline) = Instant::now().checked_add(config.budget) else {
            return Err(suggestions);
        };
        Ok(Self {
            deadline,
            cancellation,
            max_remove_probes: config.max_remove_probes,
            suggestions,
        })
    }

    fn is_active(&self) -> bool {
        !self.cancellation.is_cancelled() && Instant::now() < self.deadline
    }

    fn push(&mut self, suggestion: FailureSuggestion) {
        push_unique(&mut self.suggestions, suggestion);
    }
}

impl<P, S> RouteOptimizationService<P, S>
where
    P: RoutingProvider,
{
    pub(super) fn failure_suggestions_for(
        &self,
        request: &OptimizeRouteRequest,
        error: &OptimizationServiceError,
        cancellation: &CancellationToken,
    ) -> Vec<FailureSuggestion> {
        let suggestions = service_suggestions(error, Some(request));
        let mut run = match RemediationRun::new(self.remediation, cancellation, suggestions) {
            Ok(run) => run,
            Err(suggestions) => {
                return rank_failure_suggestions(suggestions, self.remediation.max_suggestions)
            }
        };

        match error {
            OptimizationServiceError::Routing(RoutingError::MatrixBuild {
                failed_from,
                failed_to,
                ..
            }) if request.travel_time_matrix.is_none()
                && request.sparse_travel_time_matrix.is_none()
                && !request.use_euclidean_matrix =>
            {
                self.probe_alternative_modes(request, Some((failed_from, failed_to)), &mut run);
            }
            OptimizationServiceError::Routing(RoutingError::Provider(_))
                if request.travel_time_matrix.is_none()
                    && request.sparse_travel_time_matrix.is_none()
                    && !request.use_euclidean_matrix =>
            {
                self.probe_alternative_modes(request, None, &mut run);
            }
            OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => {
                self.probe_removed_locations(request, &mut run);
            }
            OptimizationServiceError::Schedule(
                ScheduleError::OutsideSingleDay | ScheduleError::TimeWindowViolation { .. },
            ) => {
                self.probe_removed_locations(request, &mut run);
            }
            _ => {}
        }
        rank_failure_suggestions(run.suggestions, self.remediation.max_suggestions)
    }

    fn probe_alternative_modes(
        &self,
        request: &OptimizeRouteRequest,
        failed_pair: Option<(&str, &str)>,
        run: &mut RemediationRun<'_>,
    ) {
        let current = request.travel_mode.unwrap_or_default();
        for candidate in [
            TravelMode::Transit,
            TravelMode::Driving,
            TravelMode::Walking,
            TravelMode::Bicycling,
        ] {
            if candidate == current || !run.is_active() {
                continue;
            }
            let mut probe = request.clone();
            probe.travel_mode = Some(candidate);
            if self
                .build_remediation_matrix_until(&probe, run.deadline)
                .is_ok()
            {
                run.push(change_travel_mode_suggestion(
                    current,
                    candidate,
                    failed_pair,
                ));
            }
        }
    }

    fn probe_removed_locations(
        &self,
        request: &OptimizeRouteRequest,
        run: &mut RemediationRun<'_>,
    ) {
        if request.locations.len() <= 2 || run.max_remove_probes == 0 {
            return;
        }
        let Some(matrix) = self
            .build_remediation_matrix_until(request, run.deadline)
            .ok()
        else {
            return;
        };
        let rows = matrix.into_rows();
        let mut first_feasible_removal = None;
        for remove_index in (1..request.locations.len() - 1).take(run.max_remove_probes) {
            if !run.is_active() {
                break;
            }
            let location_id = request.locations[remove_index].id.clone();
            let mut probe = request.clone();
            probe.locations.remove(remove_index);
            probe.use_euclidean_matrix = false;
            probe.travel_time_matrix = None;
            probe.sparse_travel_time_matrix = Some(subset_matrix(&rows, remove_index));
            if lightweight_feasibility_probe(probe, run.cancellation) {
                run.push(remove_location_suggestion(&location_id));
                first_feasible_removal.get_or_insert(location_id);
            }
        }
        if let Some(location_id) = first_feasible_removal {
            run.push(split_day_probe_suggestion(&location_id));
        }
    }

    fn build_remediation_matrix_until(
        &self,
        request: &OptimizeRouteRequest,
        deadline: Instant,
    ) -> Result<TravelTimeMatrix, OptimizationServiceError> {
        let problem = OptimizationProblem::try_from(request.clone())?;
        let euclidean_matrix = request.use_euclidean_matrix.then(|| {
            crate::euclidean::travel_time_rows(
                &request.locations,
                request.travel_mode.unwrap_or_default(),
            )
        });
        let source = match (
            request.travel_time_matrix.clone(),
            request.sparse_travel_time_matrix.clone(),
            euclidean_matrix,
        ) {
            (Some(rows), None, None) | (None, None, Some(rows)) => MatrixSource::Dense(rows),
            (None, Some(rows), None) => MatrixSource::Sparse(rows),
            (None, None, None) => MatrixSource::Provider,
            _ => unreachable!("request validation rejects multiple matrix sources"),
        };
        let (departure_time, time_zone) = routing_departure_time(request)?;
        let routing_context = self.matrix_service.routing_context(
            departure_time,
            request.travel_mode.unwrap_or_default(),
            time_zone,
            request.country_code.as_deref(),
            request.route_provider,
        );
        self.matrix_service
            .build_until(problem.locations(), source, &routing_context, deadline)
            .map_err(Into::into)
    }
}

fn subset_matrix(rows: &[Vec<Option<u32>>], removed: usize) -> Vec<Vec<Option<u32>>> {
    rows.iter()
        .enumerate()
        .filter(|(index, _)| *index != removed)
        .map(|(_, row)| {
            row.iter()
                .enumerate()
                .filter_map(|(index, value)| (index != removed).then_some(*value))
                .collect()
        })
        .collect()
}

fn lightweight_feasibility_probe(
    request: OptimizeRouteRequest,
    cancellation: &CancellationToken,
) -> bool {
    let matrix = match (
        request.travel_time_matrix.clone(),
        request.sparse_travel_time_matrix.clone(),
    ) {
        (Some(rows), None) => TravelTimeMatrix::new(rows),
        (None, Some(rows)) => TravelTimeMatrix::new_sparse(rows),
        _ => return false,
    };
    let Ok(matrix) = matrix else { return false };
    let Ok(problem) = OptimizationProblem::try_from(request) else {
        return false;
    };
    let input = SolverInput {
        matrix: &matrix,
        problem: &problem,
        cancellation,
    };
    if problem.locations().len() <= EXACT_MAX_LOCATIONS.min(10) {
        return ExactBitDpSolver::default().solve(input).is_ok();
    }
    let Ok(solution) = GreedyInitialRoute.generate(input, &mut StdRng::seed_from_u64(0)) else {
        return false;
    };
    DefaultObjectivePolicy.evaluate(input, &solution).is_ok()
}

fn push_unique(suggestions: &mut Vec<FailureSuggestion>, candidate: FailureSuggestion) {
    let duplicate = suggestions.iter().any(|existing| {
        existing.suggestion_type == candidate.suggestion_type
            && existing.location_id == candidate.location_id
            && existing.suggested_value == candidate.suggested_value
    });
    if !duplicate {
        suggestions.push(candidate);
    }
}
