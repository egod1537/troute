use crate::solver::{
    BestSolutionTracker, ObjectiveEvaluator, RunId, SolverCandidate, SolverSolution,
};

use super::strategy::solver_id_for_strategy;

/// Owns candidate ordering and the global-best bookkeeping that must advance
/// together before a candidate becomes visible to later SA warm starts.
pub(super) struct CandidateAccumulator {
    candidates: Vec<SolverCandidate>,
    best_index: Option<usize>,
    global_best_updates: u64,
    best_tracker: BestSolutionTracker,
}

impl CandidateAccumulator {
    pub(super) fn new() -> Self {
        Self::with_capacity(0)
    }

    pub(super) fn with_capacity(capacity: usize) -> Self {
        Self {
            candidates: Vec::with_capacity(capacity),
            best_index: None,
            global_best_updates: 0,
            best_tracker: BestSolutionTracker::default(),
        }
    }

    pub(super) fn consider<E: ObjectiveEvaluator>(
        &mut self,
        evaluator: &E,
        candidate: &mut SolverCandidate,
    ) -> bool {
        let candidate_index = self.candidates.len();
        let update = self.best_tracker.consider_validated(
            evaluator,
            candidate_index,
            solver_id_for_strategy(&candidate.strategy),
            RunId::new((candidate_index + 1) as u64),
            candidate,
            candidate.elapsed.as_millis().try_into().unwrap_or(u64::MAX),
        );
        let improved = update.is_some();
        if improved {
            self.best_index = Some(candidate_index);
            self.global_best_updates += 1;
        }
        candidate.metadata.improved_global_best = Some(improved);
        improved
    }

    pub(super) fn push(&mut self, candidate: SolverCandidate) {
        self.candidates.push(candidate);
    }

    pub(super) fn has_best(&self) -> bool {
        self.best_index.is_some()
    }

    pub(super) fn warm_start(&self) -> Option<SolverSolution> {
        self.best_index
            .and_then(|index| self.candidates[index].route.clone())
    }

    pub(super) fn into_parts(self) -> (Vec<SolverCandidate>, u64) {
        (self.candidates, self.global_best_updates)
    }
}

/// Serializes concurrent strategy completions and forwards only strict objective
/// improvements. This observes real work; final deterministic selection remains
/// owned by CandidateAccumulator and CandidateSelector.
pub(super) struct PreviewObserver<'a, E> {
    pub evaluator: &'a E,
    pub observer: &'a dyn crate::solver::SolverProgressObserver,
    pub best: std::sync::Mutex<Option<crate::solver::SolutionMetrics>>,
}

impl<E: ObjectiveEvaluator> crate::solver::SolverProgressObserver for PreviewObserver<'_, E> {
    fn on_progress(&self, event: crate::solver::SolverProgressEvent) {
        self.observer.on_progress(event);
    }

    fn is_enabled(&self) -> bool {
        self.observer.is_enabled()
    }

    fn on_candidate(&self, candidate: &SolverCandidate) {
        if !self.is_enabled() || !candidate.feasible {
            return;
        }
        let (Some(solution), Some(metrics)) = (&candidate.route, candidate.objective_score) else {
            return;
        };
        let mut best = self
            .best
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if best
            .as_ref()
            .is_some_and(|current| !self.evaluator.compare(&metrics, current).is_lt())
        {
            return;
        }
        *best = Some(metrics);
        self.observer.on_best_solution(solution, &metrics);
    }
}
