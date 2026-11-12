//! Bounded, single-change repairs against the already acquired static matrix.
use std::time::{Duration, Instant};

use serde_json::json;

use crate::{
    domain::{OptimizationProblem, TimeOfDay},
    failure::{
        rank_failure_suggestions, FailureSuggestion, FailureSuggestionType, RemediationConfig,
        SuggestionConfidence,
    },
    matrix::TravelTimeMatrix,
    solver::{check_route_feasibility_until, SolverError, SolverInput},
    CancellationToken,
};

const MAX_BUDGET: Duration = Duration::from_millis(500);
const PROBE_BUDGET: Duration = Duration::from_millis(50);
const MAX_PROBES: usize = 64;
const STEP: u32 = 10;

pub(super) struct RecoveryResult {
    pub suggestions: Vec<FailureSuggestion>,
    pub probes: usize,
    pub budget_exhausted: bool,
}

struct ProbeRun<'a> {
    cancellation: &'a CancellationToken,
    deadline: Instant,
    probes: usize,
    budget_exhausted: bool,
}

impl ProbeRun<'_> {
    fn active(&mut self) -> bool {
        if self.cancellation.is_cancelled() {
            return false;
        }
        if self.probes >= MAX_PROBES || Instant::now() >= self.deadline {
            self.budget_exhausted = true;
            return false;
        }
        true
    }

    /// None is an unfinished probe, never an infeasibility result.
    fn probe(&mut self, problem: &OptimizationProblem, matrix: &TravelTimeMatrix) -> Option<bool> {
        if !self.active() {
            return None;
        }
        self.probes += 1;
        let deadline = self.deadline.min(Instant::now() + PROBE_BUDGET);
        match check_route_feasibility_until(
            SolverInput {
                problem,
                matrix,
                cancellation: self.cancellation,
            },
            deadline,
        ) {
            Ok(_) => Some(true),
            Err(SolverError::ProvenInfeasible) => Some(false),
            Err(_) => {
                self.budget_exhausted = true;
                None
            }
        }
    }
}

pub(super) fn find_recovery_suggestions(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    cancellation: &CancellationToken,
    config: RemediationConfig,
) -> RecoveryResult {
    let mut run = ProbeRun {
        cancellation,
        deadline: Instant::now() + config.budget.min(MAX_BUDGET),
        probes: 0,
        budget_exhausted: false,
    };
    let maximum = config.max_suggestions.min(5);
    let mut suggestions = Vec::new();
    if maximum > 0 && run.active() {
        if let Some(suggestion) = earlier_start(problem, matrix, &mut run) {
            suggestions.push(suggestion);
        }
        let mut targets: Vec<_> = (0..problem.locations().len())
            .filter(|&i| problem.locations()[i].stay_minutes() >= 2 * STEP)
            .collect();
        targets.sort_by_key(|&i| (std::cmp::Reverse(problem.locations()[i].stay_minutes()), i));
        for index in targets {
            if suggestions.len() >= maximum || !run.active() {
                break;
            }
            if let Some(suggestion) = shorter_stay(problem, matrix, index, &mut run) {
                suggestions.push(suggestion);
            }
        }
        for index in (1..problem.end_location_index())
            // Removing a location must not move or delete any pinned position.
            .filter(|&index| {
                !problem.locations()[index..]
                    .iter()
                    .any(|location| location.order_locked())
            })
            .take(config.max_remove_probes)
        {
            if suggestions.len() >= maximum || !run.active() {
                break;
            }
            if let Some(suggestion) = removed_place(problem, matrix, index, &mut run) {
                suggestions.push(suggestion);
            }
        }
    }
    RecoveryResult {
        suggestions: rank_failure_suggestions(suggestions, maximum),
        probes: run.probes,
        budget_exhausted: run.budget_exhausted,
    }
}

/// The original value is known infeasible. Keep a verified feasible value even
/// when the refinement budget ends; never treat a timeout as a failed value.
fn largest_feasible(
    mut good: u32,
    mut bad: u32,
    mut probe: impl FnMut(u32) -> Option<bool>,
) -> Option<u32> {
    if good >= bad || probe(good) != Some(true) {
        return None;
    }
    while bad - good > 1 {
        let middle = good + (bad - good) / 2;
        match probe(middle) {
            Some(true) => good = middle,
            Some(false) => bad = middle,
            None => break,
        }
    }
    Some(good)
}

fn earlier_start(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    run: &mut ProbeRun<'_>,
) -> Option<FailureSuggestion> {
    let current = problem.start_time();
    let steps = u32::from(current.minutes()) / STEP;
    if steps == 0 {
        return None;
    }
    let best = largest_feasible(0, steps, |step| {
        let start = TimeOfDay::from_minutes((step * STEP) as u16).ok()?;
        let changed = OptimizationProblem::with_policies(
            problem.locations().to_vec(),
            start,
            problem.start_policy(),
            problem.visit_policy(),
        );
        run.probe(&changed, matrix)
    })?;
    let suggested = TimeOfDay::from_minutes((best * STEP) as u16).ok()?;
    let mut result = verified(FailureSuggestionType::StartEarlier);
    result.current_value = Some(json!(current));
    result.suggested_value = Some(json!(suggested));
    result.suggested_latest_start = Some(suggested);
    result.required_shift_minutes = Some(u32::from(current.minutes() - suggested.minutes()));
    Some(result)
}

fn shorter_stay(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    index: usize,
    run: &mut ProbeRun<'_>,
) -> Option<FailureSuggestion> {
    let location = &problem.locations()[index];
    let current = location.stay_minutes();
    // Ten minutes is the app's minimum and request increment. Suggestions must
    // remain applicable when the same frontend request builder is used again.
    let best = largest_feasible(1, current / STEP, |step| {
        let mut locations = problem.locations().to_vec();
        locations[index] = locations[index].clone().with_stay_minutes(step * STEP);
        let changed = OptimizationProblem::with_policies(
            locations,
            problem.start_time(),
            problem.start_policy(),
            problem.visit_policy(),
        );
        run.probe(&changed, matrix)
    })? * STEP;
    let mut result = verified(FailureSuggestionType::ReduceStayTime);
    result.location_id = Some(location.id().to_owned());
    result.current_minutes = Some(current);
    result.suggested_max_minutes = Some(best);
    result.current_value = Some(json!(current));
    result.suggested_value = Some(json!(best));
    Some(result)
}

fn removed_place(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    index: usize,
    run: &mut ProbeRun<'_>,
) -> Option<FailureSuggestion> {
    let locations = problem
        .locations()
        .iter()
        .enumerate()
        .filter_map(|(i, location)| (i != index).then_some(location.clone()))
        .collect();
    let changed = OptimizationProblem::with_policies(
        locations,
        problem.start_time(),
        problem.start_policy(),
        problem.visit_policy(),
    );
    let indices: Vec<_> = (0..matrix.size()).filter(|&i| i != index).collect();
    let rows = indices
        .iter()
        .map(|&from| {
            indices
                .iter()
                .map(|&to| matrix.travel_minutes(from, to))
                .collect()
        })
        .collect();
    let matrix = TravelTimeMatrix::new_sparse(rows).ok()?;
    if run.probe(&changed, &matrix) != Some(true) {
        return None;
    }
    let mut result = verified(FailureSuggestionType::RemoveLocation);
    result.location_id = Some(problem.locations()[index].id().to_owned());
    Some(result)
}

fn verified(kind: FailureSuggestionType) -> FailureSuggestion {
    let mut result = FailureSuggestion::new(kind, SuggestionConfidence::Exact,
        "Rechecking the complete itinerary with this single change found a feasible route under the supplied static travel times.");
    result.feasibility_verified = true;
    result
}

#[cfg(test)]
mod tests;
