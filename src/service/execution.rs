use crate::{
    cancellation::CancellationToken,
    domain::{OptimizationProblem, TimeOfDay},
    events::{OptimizationEventReporter, ProgressStage},
    matrix::TravelTimeMatrix,
    schedule::{calculate_schedule_from, ScheduleError},
    solver::{
        RouteSolver, SolverError, SolverInput, SolverProgressEvent, SolverProgressObserver,
        SolverRunResult,
    },
};

use super::{
    check_cancelled, preparation::PreparedOptimization, OptimizationServiceError,
    ProgressCoordinator, RouteOptimizationService,
};

struct SolverProgressReporter<'a> {
    progress: &'a ProgressCoordinator<'a>,
    reporter: &'a dyn OptimizationEventReporter,
    input: SolverInput<'a>,
}

impl SolverProgressObserver for SolverProgressReporter<'_> {
    fn on_best_solution(
        &self,
        solution: &crate::solver::SolverSolution,
        metrics: &crate::solver::SolutionMetrics,
    ) {
        if self.input.cancellation.is_cancelled() {
            return;
        }
        let Ok(start) = crate::solver::selected_start_time(self.input.problem, metrics) else {
            return;
        };
        if let Ok(plan) =
            calculate_schedule_from(self.input.problem, self.input.matrix, solution, start)
        {
            self.reporter
                .preview(&crate::api::OptimizeRouteResponse::from_plan(
                    plan,
                    self.input.problem,
                    None,
                ));
        }
    }

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

impl<P, S> RouteOptimizationService<P, S>
where
    S: RouteSolver,
{
    pub(super) fn solve_prepared(
        &self,
        prepared: &PreparedOptimization,
        matrix: &TravelTimeMatrix,
        reporter: &dyn OptimizationEventReporter,
        progress: &ProgressCoordinator<'_>,
        cancellation: &CancellationToken,
    ) -> Result<(SolverRunResult, TimeOfDay), OptimizationServiceError> {
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
        let input = SolverInput {
            matrix,
            problem: &prepared.problem,
            cancellation,
        };
        let solver_progress = SolverProgressReporter {
            progress,
            reporter,
            input,
        };
        let mut result = match self.solver.solve_with_portfolio_options(
            input,
            &solver_progress,
            prepared.portfolio_experiment.as_ref(),
        ) {
            Ok(result) => result,
            Err(SolverError::Cancelled) => return Err(OptimizationServiceError::Cancelled),
            Err(SolverError::NoFeasibleRoute) => {
                if let Some(error) = diagnose_input_order_schedule(&prepared.problem, matrix) {
                    return Err(error.into());
                }
                return Err(SolverError::NoFeasibleRoute.into());
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(telemetry) = result
            .diagnostics
            .as_mut()
            .and_then(|diagnostics| diagnostics.portfolio_telemetry.as_mut())
        {
            for event in &mut telemetry.events {
                event.job_id.clone_from(&prepared.telemetry_job_id);
            }
        }
        if let Some(telemetry) = result
            .diagnostics
            .as_ref()
            .and_then(|diagnostics| diagnostics.portfolio_telemetry.as_ref())
        {
            for event in &telemetry.events {
                reporter.solver_telemetry(event);
            }
        }
        let candidate_count = result
            .diagnostics
            .as_ref()
            .map_or(1, |diagnostics| diagnostics.candidates.len());
        progress.report(
            ProgressStage::SelectingBestCandidate,
            ProgressStage::SelectingBestCandidate.range().end,
            format!("Selected the best feasible route from {candidate_count} candidates."),
        );
        let start_time = match self.solver.selected_start_time(input, &result.solution) {
            Ok(start_time) => start_time,
            Err(SolverError::Cancelled) => return Err(OptimizationServiceError::Cancelled),
            Err(error) => return Err(error.into()),
        };
        Ok((result, start_time))
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
