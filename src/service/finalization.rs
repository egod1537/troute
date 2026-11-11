use crate::{
    api::OptimizeRouteResponse,
    cancellation::CancellationToken,
    domain::TimeOfDay,
    events::{OptimizationEventReporter, ProgressStage},
    matrix::TravelTimeMatrix,
    result_debug::shuffle_solution,
    routing::RoutingProvider,
    schedule::calculate_schedule_from,
    solver::SolverRunResult,
};

use super::{
    check_cancelled, preparation::PreparedOptimization, revalidation::SelectedRouteRevalidation,
    routing::ResolvedRouting, OptimizationServiceError, ProgressCoordinator,
    RouteOptimizationService,
};

pub(super) struct FinalizationInput<'a> {
    pub(super) prepared: &'a PreparedOptimization,
    pub(super) routing: &'a ResolvedRouting,
    pub(super) matrix: &'a TravelTimeMatrix,
    pub(super) solver_result: SolverRunResult,
    pub(super) selected_start_time: TimeOfDay,
}

impl<P, S> RouteOptimizationService<P, S>
where
    P: RoutingProvider,
{
    pub(super) fn finalize_response(
        &self,
        input: FinalizationInput<'_>,
        reporter: &dyn OptimizationEventReporter,
        progress: &ProgressCoordinator<'_>,
        cancellation: &CancellationToken,
    ) -> Result<OptimizeRouteResponse, OptimizationServiceError> {
        let FinalizationInput {
            prepared,
            routing,
            matrix,
            solver_result,
            selected_start_time,
        } = input;
        check_cancelled(cancellation)?;
        progress.report(
            ProgressStage::Scheduling,
            ProgressStage::Scheduling.range().start,
            format!(
                "Calculating arrival and departure times for {} locations.",
                prepared.problem.locations().len()
            ),
        );
        check_cancelled(cancellation)?;
        let mut rendered_solution = prepared
            .shuffle_seed
            .map(|seed| shuffle_solution(&solver_result.solution, seed))
            .unwrap_or_else(|| solver_result.solution.clone());
        prepared
            .problem
            .restore_order_locks(&mut rendered_solution.visit_order);
        if !prepared
            .problem
            .preserves_order_locks(&rendered_solution.visit_order)
        {
            rendered_solution = solver_result.solution.clone();
        }
        let static_plan = calculate_schedule_from(
            &prepared.problem,
            matrix,
            &rendered_solution,
            selected_start_time,
        )?;
        check_cancelled(cancellation)?;
        let (plan, time_dependent_revalidation) = if let (true, Some(routing_context)) =
            (prepared.dated_routing, routing.context.as_ref())
        {
            self.revalidate_selected_route(SelectedRouteRevalidation {
                problem: &prepared.problem,
                matrix,
                solution: &rendered_solution,
                selected_start_time,
                static_plan,
                routing_context,
                cancellation,
            })?
        } else {
            (static_plan, None)
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
            &prepared.problem,
            solver_result.diagnostics.as_ref(),
        );
        response.set_provider_metadata(
            routing.selection.as_ref(),
            prepared.country_code.clone(),
            prepared.travel_mode,
        );
        if let Some(context) = &routing.context {
            if let Some(departure_time) = context.departure_time {
                response.set_routing_time_metadata(departure_time, context.timezone.clone());
            }
        }
        if let Some(revalidation) = time_dependent_revalidation {
            response.set_time_dependent_revalidation(revalidation);
        }
        check_cancelled(cancellation)?;
        reporter.result(&response);
        // Persist the full, versioned diagnostics first, then preserve the
        // legacy response shape for ordinary traffic. Explicit opt-in
        // experiment requests receive the inline summary and Run IDs.
        if prepared.portfolio_experiment.is_none() {
            response.strip_portfolio_extensions();
        }
        check_cancelled(cancellation)?;
        Ok(response)
    }
}
