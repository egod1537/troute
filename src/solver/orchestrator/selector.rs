use std::cmp::Ordering;

use crate::solver::{ObjectiveEvaluator, SolverCandidate, SolverError};

/// Applies one objective evaluator to every candidate and deterministically
/// keeps the first registered strategy when all objective fields tie.
pub struct CandidateSelector<'a, E> {
    evaluator: &'a E,
}

impl<'a, E: ObjectiveEvaluator> CandidateSelector<'a, E> {
    pub fn new(evaluator: &'a E) -> Self {
        Self { evaluator }
    }

    pub fn rank(&self, candidates: &mut [SolverCandidate]) {
        candidates.sort_by(|left, right| self.compare_candidates(left, right));
    }

    pub fn select<'b>(
        &self,
        candidates: &'b [SolverCandidate],
    ) -> Result<&'b SolverCandidate, SolverError> {
        candidates
            .iter()
            .find(|candidate| candidate.feasible && candidate.objective_score.is_some())
            .ok_or(SolverError::NoFeasibleRoute)
    }

    fn compare_candidates(&self, left: &SolverCandidate, right: &SolverCandidate) -> Ordering {
        match (left.feasible, right.feasible) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (true, true) => match (&left.objective_score, &right.objective_score) {
                (Some(left), Some(right)) => self.evaluator.compare(left, right),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
            (false, false) => Ordering::Equal,
        }
    }
}
