use std::time::{Duration, Instant};

use crate::solver::{
    ObjectiveEvaluator, PortfolioTelemetrySummary, SolverCandidate, SolverError,
    SolverProgressEvent, SolverProgressObserver, SolverSolution,
};

use super::{
    strategy::{duration_ms, report_progress, solver_id_for_strategy},
    CandidateSelector,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorSolveResult {
    pub solution: SolverSolution,
    pub selected_strategy: String,
    pub candidates: Vec<SolverCandidate>,
    pub total_budget_ms: u64,
    pub total_elapsed_ms: u64,
    pub baseline_elapsed_ms: u64,
    pub sa_elapsed_ms: u64,
    pub sa_run_count: u64,
    pub global_best_updates: u64,
    pub termination_reason: String,
    pub portfolio_telemetry: Option<PortfolioTelemetrySummary>,
}

pub(super) struct SolveResultStats<'a> {
    pub total_budget: Duration,
    pub started: Instant,
    pub baseline_elapsed: Duration,
    pub sa_elapsed: Duration,
    pub sa_run_count: u64,
    pub global_best_updates: u64,
    pub termination_reason: &'a str,
}

/// Ranks accumulated candidates and turns the winning candidate into the
/// stable public result shape.
pub(super) struct SolveResultBuilder<'a, E> {
    evaluator: &'a E,
    observer: &'a dyn SolverProgressObserver,
}

impl<'a, E: ObjectiveEvaluator> SolveResultBuilder<'a, E> {
    pub(super) fn new(evaluator: &'a E, observer: &'a dyn SolverProgressObserver) -> Self {
        Self {
            evaluator,
            observer,
        }
    }

    pub(super) fn build(
        &self,
        mut candidates: Vec<SolverCandidate>,
        stats: SolveResultStats<'_>,
    ) -> Result<OrchestratorSolveResult, SolverError> {
        assign_run_identity(&mut candidates);
        report_progress(self.observer, || {
            SolverProgressEvent::SelectionStarted(candidates.len())
        });
        let selector = CandidateSelector::new(self.evaluator);
        selector.rank(&mut candidates);
        let selected = selector.select(&candidates)?;
        let solution = selected.route.clone().ok_or(SolverError::NoFeasibleRoute)?;
        let selected_strategy = selected.strategy.clone();
        Ok(OrchestratorSolveResult {
            solution,
            selected_strategy,
            candidates,
            total_budget_ms: duration_ms(stats.total_budget),
            total_elapsed_ms: duration_ms(stats.started.elapsed()),
            baseline_elapsed_ms: duration_ms(stats.baseline_elapsed),
            sa_elapsed_ms: duration_ms(stats.sa_elapsed),
            sa_run_count: stats.sa_run_count,
            global_best_updates: stats.global_best_updates,
            termination_reason: stats.termination_reason.to_owned(),
            portfolio_telemetry: None,
        })
    }
}

pub(super) fn assign_run_identity(candidates: &mut [SolverCandidate]) {
    for (index, candidate) in candidates.iter_mut().enumerate() {
        candidate
            .metadata
            .solver_id
            .get_or_insert_with(|| solver_id_for_strategy(&candidate.strategy).to_string());
        candidate.metadata.run_id.get_or_insert((index + 1) as u64);
    }
}
