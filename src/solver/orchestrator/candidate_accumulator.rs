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
