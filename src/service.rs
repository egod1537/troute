use std::{sync::Mutex, time::Instant};

use rand::{rngs::StdRng, SeedableRng};
use thiserror::Error;

use crate::{
    api::{OptimizeRouteRequest, OptimizeRouteResponse, RequestValidationError},
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    events::{
        NoopOptimizationEventReporter, OptimizationErrorCode, OptimizationEventReporter,
        ProgressStage,
    },
    failure::{
        change_travel_mode_suggestion, rank_failure_suggestions, remove_location_suggestion,
        service_suggestions, split_day_probe_suggestion, FailureSuggestion, RemediationConfig,
    },
    matrix::TravelTimeMatrix,
    result_debug::shuffle_solution,
    routing::{
        normalize_country_code, RouteProviderPolicyDiagnostics, RouteProviderSelection,
        RoutingContext, RoutingError, RoutingProgressObserver, RoutingProvider,
    },
    schedule::{calculate_schedule_from, ScheduleError},
    solver::{
        DefaultObjectivePolicy, ExactBitDpSolver, GreedyInitialRoute, InitialRouteGenerator,
        ObjectiveEvaluator, RouteSolver, SolverError, SolverInput, SolverProgressEvent,
        SolverProgressObserver, EXACT_MAX_LOCATIONS,
    },
};

const ROUTING_DEPARTURE_LEAD_MINUTES: i64 = 5;

struct ProgressState {
    stage: Option<ProgressStage>,
    progress: u8,
}

struct ProgressCoordinator<'a> {
    reporter: &'a dyn OptimizationEventReporter,
    state: Mutex<ProgressState>,
}

impl<'a> ProgressCoordinator<'a> {
    fn new(reporter: &'a dyn OptimizationEventReporter) -> Self {
        Self {
            reporter,
            state: Mutex::new(ProgressState {
                stage: None,
                progress: 0,
            }),
        }
    }

    fn report(&self, stage: ProgressStage, progress: u8, message: impl AsRef<str>) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.stage.is_some_and(|current| stage < current) {
            return;
        }
        let progress = state.progress.max(progress.min(100));
        state.stage = Some(stage);
        state.progress = progress;
        self.reporter
            .progress(stage, progress, Some(message.as_ref()));
    }
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

struct SolverProgressReporter<'a> {
    progress: &'a ProgressCoordinator<'a>,
}

impl SolverProgressObserver for SolverProgressReporter<'_> {
    fn on_progress(&self, event: SolverProgressEvent) {
        match event {
            SolverProgressEvent::StrategyStarted(name) => self.progress.report(
                ProgressStage::OptimizingRoute,
                66,
                format!("Running solver strategy {name}."),
            ),
            SolverProgressEvent::StrategyCompleted(name) => self.progress.report(
                ProgressStage::OptimizingRoute,
                68,
                format!("Completed solver strategy {name}."),
            ),
            SolverProgressEvent::AnnealingRunStarted(index) => self.progress.report(
                ProgressStage::OptimizingRoute,
                70_u8.saturating_add(u8::try_from(index.saturating_sub(1).min(9)).unwrap_or(9)),
                format!("Running simulated annealing search {index}."),
            ),
            SolverProgressEvent::SelectionStarted(candidate_count) => self.progress.report(
                ProgressStage::SelectingBestCandidate,
                ProgressStage::SelectingBestCandidate.range().start,
                format!("Comparing {candidate_count} route candidates."),
            ),
        }
    }
}

/// Coordinates the provider -> solver -> schedule pipeline without coupling
/// any of those components to an HTTP framework.
pub struct RouteOptimizationService<P, S> {
    routing_provider: P,
    solver: S,
    remediation: RemediationConfig,
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
            remediation: RemediationConfig::default(),
        }
    }

    pub fn with_remediation_config(mut self, remediation: RemediationConfig) -> Self {
        self.remediation = remediation;
        self
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
        let result = (|| {
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
            let shuffle_options = request.debug.as_ref().and_then(|debug| {
                debug
                    .shuffle_result_route
                    .unwrap_or(false)
                    .then_some(debug.shuffle_seed)
            });
            let supplied_matrix = request.travel_time_matrix.clone();
            let travel_mode = request.travel_mode.unwrap_or_default();
            let requested_country = request.country_code.clone();
            let requested_provider = request.route_provider;
            let problem = OptimizationProblem::try_from(request)?;
            let country_code = requested_country
                .as_deref()
                .map(normalize_country_code)
                .transpose()
                .expect("request validation normalizes country codes");
            check_cancelled(cancellation)?;
            progress.report(
                ProgressStage::SelectingProvider,
                ProgressStage::SelectingProvider.range().start,
                if supplied_matrix.is_some() {
                    "Using the caller-supplied matrix; no route provider is required."
                } else {
                    "Selecting a route provider for the requested travel mode."
                },
            );
            check_cancelled(cancellation)?;
            let mut provider_selection: Option<RouteProviderSelection> = None;
            let routing_context = if supplied_matrix.is_none() {
                let mut context = RoutingContext {
                    departure_time: Some(routing_departure_time()),
                    travel_mode,
                    ..RoutingContext::default()
                };
                apply_provider_request_context(
                    &mut context,
                    country_code.as_deref(),
                    requested_provider,
                );
                provider_selection = self.routing_provider.provider_selection(&context)?;
                if let Some(selection) = &provider_selection {
                    progress.report(
                        ProgressStage::SelectingProvider,
                        ProgressStage::SelectingProvider.range().start,
                        format!("Selected {} as the route provider.", selection.provider),
                    );
                }
                Some(context)
            } else {
                None
            };
            let pair_count = problem
                .locations()
                .len()
                .saturating_mul(problem.locations().len().saturating_sub(1));
            progress.report(
                ProgressStage::PreparingMatrix,
                ProgressStage::PreparingMatrix.range().start,
                format!("Prepared {pair_count} directed travel-time pairs."),
            );
            check_cancelled(cancellation)?;
            progress.report(
                ProgressStage::FetchingTravelTimes,
                ProgressStage::FetchingTravelTimes.range().start,
                if supplied_matrix.is_some() {
                    "Loading the caller-supplied travel-time matrix.".to_owned()
                } else {
                    format!("Fetching travel times for {pair_count} directed pairs.")
                },
            );
            let matrix = match supplied_matrix {
                Some(rows) => TravelTimeMatrix::new(rows)
                    .expect("request validation guarantees a non-empty square matrix"),
                None => {
                    let routing_context = routing_context
                        .as_ref()
                        .expect("routing context exists when no matrix was supplied");
                    let routing_progress = RoutingProgressReporter {
                        progress: &progress,
                        provider: provider_selection
                            .as_ref()
                            .map(|selection| selection.provider.to_string()),
                    };
                    self.routing_provider.travel_time_matrix_with_progress(
                        problem.locations(),
                        routing_context,
                        &routing_progress,
                    )?
                }
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
            check_cancelled(cancellation)?;
            progress.report(
                ProgressStage::GeneratingCandidates,
                ProgressStage::GeneratingCandidates.range().start,
                "Generating initial route candidates.",
            );
            check_cancelled(cancellation)?;
            progress.report(
                ProgressStage::OptimizingRoute,
                ProgressStage::OptimizingRoute.range().start,
                "Optimizing route candidates.",
            );
            check_cancelled(cancellation)?;
            let solver_progress = SolverProgressReporter {
                progress: &progress,
            };
            let solver_result = match self.solver.solve_with_diagnostics_and_progress(
                SolverInput {
                    matrix: &matrix,
                    problem: &problem,
                    cancellation,
                },
                &solver_progress,
            ) {
                Ok(result) => result,
                Err(SolverError::Cancelled) => return Err(OptimizationServiceError::Cancelled),
                Err(SolverError::NoFeasibleRoute) => {
                    if let Some(schedule_error) = diagnose_input_order_schedule(&problem, &matrix) {
                        return Err(schedule_error.into());
                    }
                    return Err(SolverError::NoFeasibleRoute.into());
                }
                Err(error) => return Err(error.into()),
            };
            let candidate_count = solver_result
                .diagnostics
                .as_ref()
                .map_or(1, |diagnostics| diagnostics.candidates.len());
            progress.report(
                ProgressStage::SelectingBestCandidate,
                ProgressStage::SelectingBestCandidate.range().end,
                format!("Selected the best feasible route from {candidate_count} candidates."),
            );
            let solution = solver_result.solution;
            let selected_start_time = match self.solver.selected_start_time(
                SolverInput {
                    matrix: &matrix,
                    problem: &problem,
                    cancellation,
                },
                &solution,
            ) {
                Ok(start_time) => start_time,
                Err(SolverError::Cancelled) => return Err(OptimizationServiceError::Cancelled),
                Err(error) => return Err(error.into()),
            };
            check_cancelled(cancellation)?;
            progress.report(
                ProgressStage::Scheduling,
                ProgressStage::Scheduling.range().start,
                format!(
                    "Calculating arrival and departure times for {} locations.",
                    problem.locations().len()
                ),
            );
            check_cancelled(cancellation)?;
            let normal_plan =
                calculate_schedule_from(&problem, &matrix, &solution, selected_start_time)?;
            check_cancelled(cancellation)?;
            let plan = match shuffle_options {
                Some(seed) => {
                    let shuffled_solution = shuffle_solution(&solution, seed);
                    calculate_schedule_from(
                        &problem,
                        &matrix,
                        &shuffled_solution,
                        selected_start_time,
                    )?
                }
                None => normal_plan,
            };
            check_cancelled(cancellation)?;
            progress.report(
                ProgressStage::ValidatingSchedule,
                ProgressStage::ValidatingSchedule.range().start,
                "Validated opening hours, stays, and single-day constraints.",
            );
            check_cancelled(cancellation)?;
            progress.report(
                ProgressStage::FinalizingResult,
                ProgressStage::FinalizingResult.range().start,
                "Finalizing optimization results and diagnostics.",
            );
            let mut response = OptimizeRouteResponse::from_plan(
                plan,
                &problem,
                solver_result.diagnostics.as_ref(),
            );
            response.set_provider_metadata(provider_selection.as_ref(), country_code, travel_mode);
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

    pub fn build_travel_time_matrix(
        &self,
        request: OptimizeRouteRequest,
    ) -> Result<crate::matrix::TravelTimeMatrix, OptimizationServiceError> {
        let travel_mode = request.travel_mode.unwrap_or_default();
        let requested_country = request.country_code.clone();
        let requested_provider = request.route_provider;
        let problem = OptimizationProblem::try_from(request)?;
        let mut routing_context = RoutingContext {
            departure_time: Some(routing_departure_time()),
            travel_mode,
            ..RoutingContext::default()
        };
        apply_provider_request_context(
            &mut routing_context,
            requested_country.as_deref(),
            requested_provider,
        );
        self.routing_provider
            .travel_time_matrix(problem.locations(), &routing_context)
            .map_err(Into::into)
    }

    pub fn provider_policy_diagnostics(&self) -> Option<RouteProviderPolicyDiagnostics> {
        self.routing_provider.provider_policy_diagnostics()
    }

    pub fn failure_suggestions(
        &self,
        request: &OptimizeRouteRequest,
        error: &OptimizationServiceError,
        cancellation: &CancellationToken,
    ) -> Vec<FailureSuggestion> {
        let mut suggestions = service_suggestions(error, Some(request));
        if cancellation.is_cancelled() || self.remediation.budget.is_zero() {
            return rank_failure_suggestions(suggestions, self.remediation.max_suggestions);
        }
        let Some(deadline) = Instant::now().checked_add(self.remediation.budget) else {
            return rank_failure_suggestions(suggestions, self.remediation.max_suggestions);
        };

        match error {
            OptimizationServiceError::Routing(RoutingError::MatrixBuild {
                failed_from,
                failed_to,
                ..
            }) if request.travel_time_matrix.is_none() => {
                self.probe_alternative_modes(
                    request,
                    Some((failed_from, failed_to)),
                    deadline,
                    cancellation,
                    &mut suggestions,
                );
            }
            OptimizationServiceError::Routing(RoutingError::Provider(_))
                if request.travel_time_matrix.is_none() =>
            {
                self.probe_alternative_modes(
                    request,
                    None,
                    deadline,
                    cancellation,
                    &mut suggestions,
                );
            }
            OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => {
                self.probe_removed_locations(request, deadline, cancellation, &mut suggestions);
            }
            OptimizationServiceError::Schedule(
                ScheduleError::OutsideSingleDay | ScheduleError::TimeWindowViolation { .. },
            ) => {
                self.probe_removed_locations(request, deadline, cancellation, &mut suggestions);
            }
            _ => {}
        }
        rank_failure_suggestions(suggestions, self.remediation.max_suggestions)
    }

    fn probe_alternative_modes(
        &self,
        request: &OptimizeRouteRequest,
        failed_pair: Option<(&str, &str)>,
        deadline: Instant,
        cancellation: &CancellationToken,
        suggestions: &mut Vec<FailureSuggestion>,
    ) {
        let current = request.travel_mode.unwrap_or_default();
        for candidate in [
            crate::routing::TravelMode::Transit,
            crate::routing::TravelMode::Driving,
            crate::routing::TravelMode::Walking,
            crate::routing::TravelMode::Bicycling,
        ] {
            if candidate == current || cancellation.is_cancelled() || Instant::now() >= deadline {
                continue;
            }
            let mut probe = request.clone();
            probe.travel_mode = Some(candidate);
            if self
                .build_remediation_matrix_until(&probe, deadline)
                .is_ok()
            {
                push_unique(
                    suggestions,
                    change_travel_mode_suggestion(current, candidate, failed_pair),
                );
            }
        }
    }

    fn probe_removed_locations(
        &self,
        request: &OptimizeRouteRequest,
        deadline: Instant,
        cancellation: &CancellationToken,
        suggestions: &mut Vec<FailureSuggestion>,
    ) {
        if request.locations.len() <= 2 || self.remediation.max_remove_probes == 0 {
            return;
        }
        let Some(matrix) = self.build_remediation_matrix_until(request, deadline).ok() else {
            return;
        };
        let rows = matrix.into_rows();
        let mut first_feasible_removal = None;
        for remove_index in
            (1..request.locations.len() - 1).take(self.remediation.max_remove_probes)
        {
            if cancellation.is_cancelled() || Instant::now() >= deadline {
                break;
            }
            let location_id = request.locations[remove_index].id.clone();
            let mut probe = request.clone();
            probe.locations.remove(remove_index);
            probe.travel_time_matrix = Some(subset_matrix(&rows, remove_index));
            if lightweight_feasibility_probe(probe, cancellation) {
                push_unique(suggestions, remove_location_suggestion(&location_id));
                first_feasible_removal.get_or_insert(location_id);
            }
        }
        if let Some(location_id) = first_feasible_removal {
            push_unique(suggestions, split_day_probe_suggestion(&location_id));
        }
    }

    fn build_remediation_matrix_until(
        &self,
        request: &OptimizeRouteRequest,
        deadline: Instant,
    ) -> Result<TravelTimeMatrix, OptimizationServiceError> {
        if let Some(rows) = request.travel_time_matrix.clone() {
            return TravelTimeMatrix::new(rows)
                .map_err(RoutingError::InvalidMatrix)
                .map_err(Into::into);
        }
        let problem = OptimizationProblem::try_from(request.clone())?;
        let mut routing_context = RoutingContext {
            departure_time: Some(routing_departure_time()),
            travel_mode: request.travel_mode.unwrap_or_default(),
            ..RoutingContext::default()
        };
        apply_provider_request_context(
            &mut routing_context,
            request.country_code.as_deref(),
            request.route_provider,
        );
        self.routing_provider
            .travel_time_matrix_until(problem.locations(), &routing_context, deadline)
            .map_err(Into::into)
    }
}

fn diagnose_input_order_schedule(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
) -> Option<ScheduleError> {
    let solution = crate::solver::SolverSolution {
        visit_order: (0..problem.locations().len()).collect(),
    };
    calculate_schedule_from(problem, matrix, &solution, problem.start_time())
        .err()
        .and_then(|error| {
            matches!(error, ScheduleError::TimeWindowViolation { .. }).then_some(error)
        })
}

fn subset_matrix(rows: &[Vec<u32>], removed: usize) -> Vec<Vec<u32>> {
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
    let Some(rows) = request.travel_time_matrix.clone() else {
        return false;
    };
    let Ok(matrix) = TravelTimeMatrix::new(rows) else {
        return false;
    };
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

fn routing_departure_time() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() + chrono::Duration::minutes(ROUTING_DEPARTURE_LEAD_MINUTES)
}

fn apply_provider_request_context(
    context: &mut RoutingContext,
    country_code: Option<&str>,
    provider: Option<crate::routing::RouteProviderName>,
) {
    if let Some(country_code) = country_code {
        context
            .options
            .insert("countryCode".to_owned(), country_code.to_owned());
    }
    if let Some(provider) = provider {
        context
            .options
            .insert("routeProviderOverride".to_owned(), provider.to_string());
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
    use std::{
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::*;
    use crate::{
        development::{DevelopmentRouteSolver, DevelopmentRoutingProvider},
        domain::Location,
        events::{OptimizationErrorCode, ProgressStage},
        matrix::TravelTimeMatrix,
        routing::TravelMode,
        solver::{
            ExactBitDpSolver, SimulatedAnnealingConfig, SolverOrchestrator,
            SolverOrchestratorConfig, SolverSolution,
        },
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
        messages: Mutex<Vec<Option<String>>>,
    }

    impl InMemoryReporter {
        fn events(&self) -> Vec<RecordedEvent> {
            self.events.lock().unwrap().clone()
        }

        fn messages(&self) -> Vec<Option<String>> {
            self.messages.lock().unwrap().clone()
        }
    }

    impl OptimizationEventReporter for InMemoryReporter {
        fn progress(&self, stage: ProgressStage, progress: u8, message: Option<&str>) {
            self.events
                .lock()
                .unwrap()
                .push(RecordedEvent::Progress(stage, progress));
            self.messages
                .lock()
                .unwrap()
                .push(message.map(str::to_owned));
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
            _context: &RoutingContext,
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
            _context: &RoutingContext,
        ) -> Result<TravelTimeMatrix, RoutingError> {
            TravelTimeMatrix::new(vec![vec![0]])
                .map_err(|error| RoutingError::Provider(error.to_string()))
        }
    }

    struct CapturingRoutingProvider {
        modes: Arc<Mutex<Vec<TravelMode>>>,
    }

    impl RoutingProvider for CapturingRoutingProvider {
        fn travel_time_matrix(
            &self,
            _locations: &[Location],
            context: &RoutingContext,
        ) -> Result<TravelTimeMatrix, RoutingError> {
            self.modes.lock().unwrap().push(context.travel_mode);
            TravelTimeMatrix::new(vec![vec![0, 15], vec![15, 0]])
                .map_err(RoutingError::InvalidMatrix)
        }
    }

    struct NoFeasibleSolver;

    impl RouteSolver for NoFeasibleSolver {
        fn solve(&self, _input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
            Err(SolverError::NoFeasibleRoute)
        }
    }

    struct AlternativeModeRoutingProvider {
        modes: Arc<Mutex<Vec<TravelMode>>>,
    }

    impl RoutingProvider for AlternativeModeRoutingProvider {
        fn travel_time_matrix(
            &self,
            _locations: &[Location],
            context: &RoutingContext,
        ) -> Result<TravelTimeMatrix, RoutingError> {
            self.modes.lock().unwrap().push(context.travel_mode);
            if context.travel_mode == TravelMode::Walking {
                TravelTimeMatrix::new(vec![vec![0, 10], vec![10, 0]])
                    .map_err(RoutingError::InvalidMatrix)
            } else {
                Err(RoutingError::Provider(format!(
                    "{} route unavailable",
                    context.travel_mode.as_provider_value()
                )))
            }
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

        let events = reporter.events();
        assert_eq!(events.last(), Some(&RecordedEvent::Result(response)));
        let progress: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                RecordedEvent::Progress(stage, progress) => Some((*stage, *progress)),
                _ => None,
            })
            .collect();
        assert!(progress.windows(2).all(|pair| pair[0].1 <= pair[1].1));
        let mut stages: Vec<_> = progress.iter().map(|(stage, _)| *stage).collect();
        stages.dedup();
        assert_eq!(
            stages,
            [
                ProgressStage::Accepted,
                ProgressStage::ValidatingRequest,
                ProgressStage::SelectingProvider,
                ProgressStage::PreparingMatrix,
                ProgressStage::FetchingTravelTimes,
                ProgressStage::BuildingMatrix,
                ProgressStage::GeneratingCandidates,
                ProgressStage::OptimizingRoute,
                ProgressStage::SelectingBestCandidate,
                ProgressStage::Scheduling,
                ProgressStage::ValidatingSchedule,
                ProgressStage::FinalizingResult,
            ]
        );
        assert!(reporter.messages().iter().all(|message| message
            .as_deref()
            .is_some_and(|message| !message.is_empty())));
    }

    #[test]
    fn exact_solver_start_selection_reaches_the_schedule() {
        let service =
            RouteOptimizationService::new(DevelopmentRoutingProvider, ExactBitDpSolver::default());
        let reporter = InMemoryReporter::default();
        let response = service
            .optimize_with_reporter(valid_request(), &reporter)
            .unwrap();

        assert_eq!(
            response.route[0].departure_time.unwrap().to_string(),
            "23:30"
        );
        assert_eq!(response.route[1].arrival_time.to_string(), "23:45");
        assert!(reporter.messages().iter().any(|message| {
            message
                .as_deref()
                .is_some_and(|message| message.contains("exact_bit_dp"))
        }));
    }

    #[test]
    fn exact_solver_honors_all_start_policies() {
        let service =
            RouteOptimizationService::new(DevelopmentRoutingProvider, ExactBitDpSolver::default());

        let mut fixed = serde_json::to_value(valid_request()).unwrap();
        fixed["start_policy"] = serde_json::json!("FIXED");
        fixed["start_time"] = serde_json::json!("09:05");
        let fixed = service
            .optimize(serde_json::from_value(fixed).unwrap())
            .unwrap();
        assert_eq!(fixed.selected_start_time.unwrap().to_string(), "09:05");
        assert_eq!(fixed.route[0].departure_time.unwrap().to_string(), "09:05");

        let mut earliest = serde_json::to_value(valid_request()).unwrap();
        earliest["start_policy"] = serde_json::json!("EARLIEST");
        earliest.as_object_mut().unwrap().remove("start_time");
        let earliest = service
            .optimize(serde_json::from_value(earliest).unwrap())
            .unwrap();
        assert_eq!(earliest.selected_start_time.unwrap().to_string(), "00:00");

        let mut latest = serde_json::to_value(valid_request()).unwrap();
        latest["start_policy"] = serde_json::json!("LATEST");
        latest.as_object_mut().unwrap().remove("start_time");
        let latest = service
            .optimize(serde_json::from_value(latest).unwrap())
            .unwrap();
        assert_eq!(latest.selected_start_time.unwrap().to_string(), "23:30");
    }

    #[test]
    fn best_candidate_objective_matches_the_returned_schedule() {
        let solver = SolverOrchestrator::new(SolverOrchestratorConfig::default()).unwrap();
        let service = RouteOptimizationService::new(DevelopmentRoutingProvider, solver);
        let response = service.optimize(valid_request()).unwrap();
        let last_departure = response.route.last().unwrap().departure_time.unwrap();
        let total_wait: u32 = response
            .route
            .iter()
            .map(|stop| stop.wait_minutes.unwrap())
            .sum();
        let best = response
            .solver_candidates
            .as_ref()
            .unwrap()
            .iter()
            .find(|candidate| candidate.best)
            .unwrap();
        let score = best.objective_score.unwrap();

        assert_eq!(best.route, ["A", "B"]);
        assert_eq!(score.start_time, response.selected_start_time);
        assert_eq!(score.finish_time, last_departure);
        assert_eq!(score.travel_minutes, response.total_travel_minutes);
        assert_eq!(score.wait_minutes, total_wait);
    }

    #[test]
    fn orchestrator_diagnostics_reach_the_api_response() {
        let solver = SolverOrchestrator::new(SolverOrchestratorConfig {
            exact_limit: 1,
            total_budget: Duration::from_millis(30),
            sa_chunk: Duration::from_millis(5),
            deadline_safety_margin: Duration::from_millis(1),
            sa_config: SimulatedAnnealingConfig {
                iteration_limit: Some(1),
                ..SimulatedAnnealingConfig::default()
            },
            ..SolverOrchestratorConfig::default()
        })
        .unwrap();
        let service = RouteOptimizationService::new(DevelopmentRoutingProvider, solver);
        let response = service.optimize(valid_request()).unwrap();
        let candidates = response.solver_candidates.unwrap();

        assert!(!candidates
            .iter()
            .any(|candidate| candidate.strategy == "exact_bit_dp"));
        let clustered = candidates
            .iter()
            .find(|candidate| candidate.strategy == "clustered")
            .unwrap();
        assert_eq!(clustered.metadata.cluster_count, Some(0));
        assert_eq!(clustered.metadata.cluster_sizes, Some(Vec::new()));
        assert_eq!(
            clustered.metadata.cluster_strategy.as_deref(),
            Some("directed_nearest_neighbor")
        );
        assert_eq!(
            clustered.metadata.cluster_order_strategy.as_deref(),
            Some("greedy_bridge")
        );
        assert!(clustered.metadata.score_before_improvement.is_some());
        assert!(clustered.metadata.score_after_improvement.is_some());
        assert_eq!(clustered.metadata.swap_enabled, Some(true));
        assert_eq!(clustered.metadata.relocate_enabled, Some(false));
        assert_eq!(clustered.metadata.two_opt_enabled, Some(false));
        let mst = candidates
            .iter()
            .find(|candidate| candidate.strategy == "mst_double_tree")
            .unwrap();
        assert_eq!(
            mst.metadata.symmetric_distance_strategy.as_deref(),
            Some("average_bidirectional")
        );
        assert_eq!(mst.metadata.mst_edge_count, Some(1));
        assert_eq!(mst.metadata.mst_edges.as_ref().unwrap().len(), 1);
        assert_eq!(
            mst.metadata.euler_tour.as_ref().unwrap(),
            &vec!["A".to_owned(), "B".to_owned(), "A".to_owned()]
        );
        assert_eq!(
            mst.metadata.shortcut_route.as_ref().unwrap(),
            &vec!["A".to_owned(), "B".to_owned()]
        );
        let christofides = candidates
            .iter()
            .find(|candidate| candidate.strategy == "christofides")
            .unwrap();
        assert_eq!(christofides.metadata.mst_edge_count, Some(1));
        assert_eq!(christofides.metadata.odd_vertex_count, Some(2));
        assert_eq!(
            christofides.metadata.odd_vertices.as_ref().unwrap(),
            &vec!["A".to_owned(), "B".to_owned()]
        );
        assert_eq!(
            christofides.metadata.matching_strategy.as_deref(),
            Some("bit_dp")
        );
        assert_eq!(
            christofides.metadata.matching_pairs.as_ref().unwrap().len(),
            1
        );
        assert_eq!(
            christofides.metadata.shortcut_route.as_ref().unwrap(),
            &vec!["A".to_owned(), "B".to_owned()]
        );
        let sa_greedy = candidates
            .iter()
            .find(|candidate| candidate.strategy.starts_with("sa_greedy_"))
            .unwrap();
        assert_eq!(
            sa_greedy.metadata.initial_strategy.as_deref(),
            Some("greedy")
        );
        assert_eq!(
            sa_greedy.metadata.initial_route.as_ref().unwrap(),
            &vec!["A".to_owned(), "B".to_owned()]
        );
        assert_eq!(
            sa_greedy.metadata.final_route.as_ref().unwrap(),
            &vec!["A".to_owned(), "B".to_owned()]
        );
        assert_eq!(sa_greedy.metadata.iteration_count, Some(0));
        assert_eq!(sa_greedy.metadata.best_feasible, Some(true));
        assert_eq!(
            candidates.iter().filter(|candidate| candidate.best).count(),
            1
        );
        assert!(candidates
            .iter()
            .any(|candidate| candidate.strategy.starts_with("sa_greedy_")));
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

        let events = reporter.events();
        assert_eq!(
            events.last(),
            Some(&RecordedEvent::Error(
                OptimizationErrorCode::RoutingUnavailable
            ))
        );
        assert!(matches!(
            events.get(events.len() - 2),
            Some(RecordedEvent::Progress(
                ProgressStage::FetchingTravelTimes,
                20
            ))
        ));
    }

    #[test]
    fn supplied_matrix_bypasses_the_routing_provider() {
        let service = RouteOptimizationService::new(FailingRoutingProvider, DevelopmentRouteSolver);
        let mut request = valid_request();
        request.travel_mode = Some(TravelMode::Driving);
        request.locations[0].place_id.clear();
        request.locations[1].place_id.clear();
        request.travel_time_matrix = Some(vec![vec![0, 7], vec![9, 0]]);

        let response = service.optimize(request).unwrap();

        assert_eq!(response.total_travel_minutes, 7);
    }

    #[test]
    fn request_travel_mode_reaches_routing_context_and_defaults_to_transit() {
        for (requested, expected) in [
            (None, TravelMode::Transit),
            (Some(TravelMode::Transit), TravelMode::Transit),
            (Some(TravelMode::Driving), TravelMode::Driving),
            (Some(TravelMode::Walking), TravelMode::Walking),
            (Some(TravelMode::Bicycling), TravelMode::Bicycling),
        ] {
            let modes = Arc::new(Mutex::new(Vec::new()));
            let service = RouteOptimizationService::new(
                CapturingRoutingProvider {
                    modes: Arc::clone(&modes),
                },
                DevelopmentRouteSolver,
            );
            let mut request = valid_request();
            request.travel_mode = requested;

            service.optimize(request).unwrap();

            assert_eq!(*modes.lock().unwrap(), [expected]);
        }
    }

    #[test]
    fn no_feasible_route_reports_the_contract_error_code() {
        let service = RouteOptimizationService::new(FixedRoutingProvider, NoFeasibleSolver);
        let reporter = InMemoryReporter::default();

        assert!(service
            .optimize_with_reporter(valid_request(), &reporter)
            .is_err());

        let events = reporter.events();
        assert_eq!(
            events.last(),
            Some(&RecordedEvent::Error(
                OptimizationErrorCode::NoFeasibleRoute
            ))
        );
        assert!(events.iter().rev().skip(1).any(|event| matches!(
            event,
            RecordedEvent::Progress(ProgressStage::OptimizingRoute, _)
        )));
    }

    #[test]
    fn routing_remediation_only_returns_modes_verified_by_a_real_query() {
        let modes = Arc::new(Mutex::new(Vec::new()));
        let service = RouteOptimizationService::new(
            AlternativeModeRoutingProvider {
                modes: Arc::clone(&modes),
            },
            UnusedSolver,
        );
        let mut request = valid_request();
        request.travel_mode = Some(TravelMode::Transit);
        let error = service.optimize(request.clone()).unwrap_err();

        let suggestions = service.failure_suggestions(&request, &error, &CancellationToken::new());
        let alternatives = suggestions
            .iter()
            .filter(|suggestion| {
                suggestion.suggestion_type
                    == crate::failure::FailureSuggestionType::ChangeTravelMode
            })
            .collect::<Vec<_>>();

        assert_eq!(alternatives.len(), 1);
        assert_eq!(
            alternatives[0].current_value,
            Some(serde_json::json!("TRANSIT"))
        );
        assert_eq!(
            alternatives[0].suggested_value,
            Some(serde_json::json!("WALKING"))
        );
        assert_eq!(
            *modes.lock().unwrap(),
            [
                TravelMode::Transit,
                TravelMode::Driving,
                TravelMode::Walking,
                TravelMode::Bicycling
            ]
        );
    }

    #[test]
    fn removal_and_split_suggestions_require_a_successful_bounded_probe() {
        let service =
            RouteOptimizationService::new(DevelopmentRoutingProvider, DevelopmentRouteSolver)
                .with_remediation_config(RemediationConfig {
                    budget: Duration::from_millis(500),
                    max_remove_probes: 1,
                    max_suggestions: 3,
                });
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "remove-probe",
            "start_policy": "FIXED",
            "start_time": "09:00",
            "locations": [
                {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"blocked","place_id":"b","open_time":"10:00","close_time":"10:10","stay_minutes":20},
                {"id":"keep","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":10},
                {"id":"D","place_id":"d","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ],
            "travel_time_matrix": [
                [0,10,10,10], [10,0,10,10], [10,10,0,10], [10,10,10,0]
            ]
        }))
        .unwrap();

        let suggestions = service.failure_suggestions(
            &request,
            &OptimizationServiceError::Solver(SolverError::NoFeasibleRoute),
            &CancellationToken::new(),
        );

        assert!(suggestions.iter().any(|suggestion| {
            suggestion.suggestion_type == crate::failure::FailureSuggestionType::RemoveLocation
                && suggestion.location_id.as_deref() == Some("blocked")
        }));
        assert!(suggestions.iter().any(|suggestion| {
            suggestion.suggestion_type == crate::failure::FailureSuggestionType::SplitDay
                && suggestion.location_id.as_deref() == Some("blocked")
        }));
        assert!(!suggestions.iter().any(|suggestion| {
            suggestion.suggestion_type == crate::failure::FailureSuggestionType::RemoveLocation
                && suggestion.location_id.as_deref() == Some("keep")
        }));
    }

    fn valid_request() -> OptimizeRouteRequest {
        serde_json::from_value(serde_json::json!({
            "job_id": "route-service-test",
            "locations": [
                {
                    "id": "A",
                    "place_id": "place-a",
                    "open_time": "00:00",
                    "close_time": "23:50",
                    "stay_minutes": 0
                },
                {
                    "id": "B",
                    "place_id": "place-b",
                    "open_time": "00:00",
                    "close_time": "23:50",
                    "stay_minutes": 0
                }
            ],
            "start_time": "09:00"
        }))
        .unwrap()
    }
}
