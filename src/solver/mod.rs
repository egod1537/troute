use std::cmp::Ordering;

use crate::domain::{OptimizationProblem, StartPolicy};

pub mod core;
pub use core::*;

pub const EXACT_MAX_LOCATIONS: usize = 15;
pub const TIME_SLOT_MINUTES: u32 = 10;

pub mod exact;
pub use exact::*;
use exact::{initial_service, minutes_to_slot_ceil, slots_per_day, transition_time};

impl<T: ObjectivePolicy> ObjectiveEvaluator for T {
    fn evaluate(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<SolutionMetrics, SolverError> {
        evaluate_solution(self, input, solution)
    }

    fn compare(&self, left: &SolutionMetrics, right: &SolutionMetrics) -> Ordering {
        ObjectivePolicy::compare(self, left, right)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultObjectivePolicy;

impl ObjectivePolicy for DefaultObjectivePolicy {
    fn score(&self, travel_minutes: u32, wait_minutes: u32) -> u64 {
        u64::from(travel_minutes) + u64::from(wait_minutes)
    }

    fn frontier_cost(&self, travel_minutes: u32, wait_minutes: u32) -> u64 {
        (u64::from(travel_minutes) << 32) | u64::from(wait_minutes)
    }

    fn compare(&self, left: &SolutionMetrics, right: &SolutionMetrics) -> Ordering {
        debug_assert_eq!(left.start_policy, right.start_policy);
        let start_order = match left.start_policy {
            StartPolicy::Fixed => Ordering::Equal,
            StartPolicy::Earliest => left.start_time_slot.cmp(&right.start_time_slot),
            StartPolicy::Latest => right.start_time_slot.cmp(&left.start_time_slot),
        };
        start_order
            .then_with(|| left.finish_time_slot.cmp(&right.finish_time_slot))
            .then_with(|| left.travel_minutes.cmp(&right.travel_minutes))
            .then_with(|| left.wait_minutes.cmp(&right.wait_minutes))
            .then_with(|| left.score.cmp(&right.score))
    }
}

/// Evaluates an already assembled route with the same time-slot and objective
/// semantics used by the exact and clustered solvers.
pub fn evaluate_solution(
    objective: &dyn ObjectivePolicy,
    input: SolverInput<'_>,
    solution: &SolverSolution,
) -> Result<SolutionMetrics, SolverError> {
    validate_visit_order(input.problem, solution)?;
    if input.matrix.size() != input.problem.locations().len() {
        return Err(SolverError::MatrixSizeMismatch {
            matrix: input.matrix.size(),
            locations: input.problem.locations().len(),
        });
    }
    let earliest = minutes_to_slot_ceil(u32::from(input.problem.start_time().minutes()));
    if earliest >= slots_per_day() || input.cancellation.is_cancelled() {
        return if input.cancellation.is_cancelled() {
            Err(SolverError::Cancelled)
        } else {
            Err(SolverError::NoFeasibleRoute)
        };
    }
    let low = earliest as u16;
    if simulate_solution(objective, &input, solution, low).is_none() {
        return Err(SolverError::NoFeasibleRoute);
    }
    if matches!(
        input.problem.start_policy(),
        StartPolicy::Fixed | StartPolicy::Earliest
    ) {
        return simulate_solution(objective, &input, solution, low)
            .ok_or(SolverError::NoFeasibleRoute);
    }
    let mut low = low;
    let mut high = (slots_per_day() - 1) as u16;
    while low < high {
        let middle = low + (high - low).div_ceil(2);
        if simulate_solution(objective, &input, solution, middle).is_some() {
            low = middle;
        } else {
            high = middle - 1;
        }
    }
    simulate_solution(objective, &input, solution, low).ok_or(SolverError::NoFeasibleRoute)
}

/// Converts slot-based metrics into the selected visit-start time exposed by the API.
/// Fixed and earliest policies retain the exact requested minute.
pub fn selected_start_time(
    problem: &OptimizationProblem,
    metrics: &SolutionMetrics,
) -> Result<crate::domain::TimeOfDay, SolverError> {
    if matches!(
        problem.start_policy(),
        StartPolicy::Fixed | StartPolicy::Earliest
    ) {
        return Ok(problem.start_time());
    }
    crate::domain::TimeOfDay::from_minutes(metrics.start_time_slot * TIME_SLOT_MINUTES as u16)
        .map_err(|error| SolverError::Failed(error.to_string()))
}

fn validate_visit_order(
    problem: &OptimizationProblem,
    solution: &SolverSolution,
) -> Result<(), SolverError> {
    let location_count = problem.locations().len();
    if solution.visit_order.len() != location_count
        || solution.visit_order.first() != Some(&problem.start_location_index())
        || solution.visit_order.last() != Some(&problem.end_location_index())
    {
        return Err(SolverError::InvalidVisitOrder);
    }
    let mut seen = vec![false; location_count];
    for &location in &solution.visit_order {
        if location >= location_count || seen[location] {
            return Err(SolverError::InvalidVisitOrder);
        }
        seen[location] = true;
    }
    Ok(())
}

fn simulate_solution(
    objective: &dyn ObjectivePolicy,
    input: &SolverInput<'_>,
    solution: &SolverSolution,
    start: u16,
) -> Option<SolutionMetrics> {
    if solution.visit_order.first() != Some(&input.problem.start_location_index())
        || solution.visit_order.last() != Some(&input.problem.end_location_index())
    {
        return None;
    }
    let initial = initial_service(input, start)?;
    let mut time_slot = u32::from(initial.departure_slot);
    let mut travel_minutes = 0_u32;
    let mut wait_minutes = initial.wait_minutes;
    for edge in solution.visit_order.windows(2) {
        let travel = input.matrix.travel_minutes(edge[0], edge[1])?;
        travel_minutes = travel_minutes.checked_add(travel)?;
        let location = &input.problem.locations()[edge[1]];
        let arrival = time_slot.checked_add(minutes_to_slot_ceil(travel))?;
        let service_start = arrival.max(minutes_to_slot_ceil(u32::from(
            location.time_window().open().minutes(),
        )));
        wait_minutes =
            wait_minutes.checked_add((service_start - arrival).checked_mul(TIME_SLOT_MINUTES)?)?;
        let finish = service_start.checked_add(minutes_to_slot_ceil(location.stay_minutes()))?;
        let close = u32::from(location.time_window().close().minutes()) / TIME_SLOT_MINUTES;
        if finish > close || finish >= slots_per_day() {
            return None;
        }
        time_slot = finish;
    }
    Some(SolutionMetrics {
        start_policy: input.problem.start_policy(),
        start_time_slot: start,
        finish_time_slot: time_slot as u16,
        travel_minutes,
        wait_minutes,
        score: objective.score(travel_minutes, wait_minutes),
    })
}

pub mod initial;
pub use initial::*;
pub mod heuristic;
pub use heuristic::*;
mod orchestrator;
pub use orchestrator::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::OptimizeRouteRequest, cancellation::CancellationToken, matrix::TravelTimeMatrix,
    };

    #[test]
    fn shared_evaluator_applies_start_wait_and_stay() {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "evaluate-start-service",
            "start_policy": "FIXED",
            "start_time": "08:30",
            "locations": [
                {"id":"A","place_id":"a","open_time":"09:00","close_time":"18:00","stay_minutes":60},
                {"id":"B","place_id":"b","open_time":"00:00","close_time":"18:00","stay_minutes":0}
            ]
        }))
        .unwrap();
        let problem: OptimizationProblem = request.try_into().unwrap();
        let matrix = TravelTimeMatrix::new(vec![vec![0, 20], vec![20, 0]]).unwrap();
        let metrics = evaluate_solution(
            &DefaultObjectivePolicy,
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            },
            &SolverSolution {
                visit_order: vec![0, 1],
            },
        )
        .unwrap();

        assert_eq!(metrics.start_time_slot, 51); // 08:30
        assert_eq!(metrics.finish_time_slot, 62); // 10:20
        assert_eq!(metrics.wait_minutes, 30);
        assert_eq!(metrics.travel_minutes, 20);
        assert_eq!(metrics.score, 50);
    }
}
