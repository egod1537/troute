use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::{
    domain::StartPolicy,
    solver::{ObjectiveEvaluator, SolutionMetrics, SolverCandidate},
};

use super::{RunId, SolverId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectiveVector {
    pub visited_optional_count: usize,
    pub visited_optional_priority: u32,
    pub start_policy: StartPolicy,
    pub selected_start: u16,
    pub finish: u16,
    pub travel: u32,
    pub wait: u32,
}

impl From<SolutionMetrics> for ObjectiveVector {
    fn from(metrics: SolutionMetrics) -> Self {
        Self {
            visited_optional_count: metrics.visited_optional_count,
            visited_optional_priority: metrics.visited_optional_priority,
            start_policy: metrics.start_policy,
            selected_start: metrics.start_time_slot,
            finish: metrics.finish_time_slot,
            travel: metrics.travel_minutes,
            wait: metrics.wait_minutes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedBest {
    pub candidate_index: usize,
    pub solver_id: SolverId,
    pub run_id: RunId,
    pub objective: SolutionMetrics,
    pub route_fingerprint: String,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BestUpdate {
    pub previous: Option<TrackedBest>,
    pub current: TrackedBest,
    pub first_feasible: bool,
}

#[derive(Debug, Default)]
pub struct BestSolutionTracker {
    best: Mutex<Option<TrackedBest>>,
}

impl BestSolutionTracker {
    pub fn consider_validated(
        &self,
        evaluator: &dyn ObjectiveEvaluator,
        candidate_index: usize,
        solver_id: SolverId,
        run_id: RunId,
        candidate: &SolverCandidate,
        updated_at_ms: u64,
    ) -> Option<BestUpdate> {
        if !candidate.feasible {
            return None;
        }
        let (Some(route), Some(objective)) = (&candidate.route, candidate.objective_score) else {
            return None;
        };
        let mut best = self
            .best
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if best
            .as_ref()
            .is_some_and(|current| !evaluator.compare(&objective, &current.objective).is_lt())
        {
            return None;
        }
        let previous = best.clone();
        let current = TrackedBest {
            candidate_index,
            solver_id,
            run_id,
            objective,
            route_fingerprint: route_fingerprint(&route.visit_order),
            updated_at_ms,
        };
        *best = Some(current.clone());
        Some(BestUpdate {
            first_feasible: previous.is_none(),
            previous,
            current,
        })
    }

    pub fn best(&self) -> Option<TrackedBest> {
        self.best
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub fn route_fingerprint(route: &[usize]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for index in route {
        for byte in index.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    format!("{hash:016x}")
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PolicyReward {
    pub value: f64,
    pub improved: bool,
    pub improved_component_rank: Option<u8>,
    pub reserved_slot_ms: u64,
}

impl PolicyReward {
    pub fn from_update(update: Option<&BestUpdate>, reserved_slot_ms: u64) -> Self {
        let improved_component_rank = update.and_then(|update| {
            let previous = update.previous.as_ref()?;
            let left = ObjectiveVector::from(update.current.objective);
            let right = ObjectiveVector::from(previous.objective);
            [
                left.visited_optional_count != right.visited_optional_count,
                left.visited_optional_priority != right.visited_optional_priority,
                left.selected_start != right.selected_start,
                left.finish != right.finish,
                left.travel != right.travel,
                left.wait != right.wait,
            ]
            .iter()
            .position(|changed| *changed)
            .map(|index| index as u8)
        });
        let improved = update.is_some();
        Self {
            value: if improved { 1.0 } else { 0.0 },
            improved,
            improved_component_rank,
            reserved_slot_ms,
        }
    }
}
