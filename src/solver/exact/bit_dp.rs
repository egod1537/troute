use crate::domain::{StartPolicy, TimeOfDay};

use super::super::{
    evaluate_solution, DefaultObjectivePolicy, ObjectivePolicy, RouteSolver, SolutionMetrics,
    SolverError, SolverInput, SolverProgressEvent, SolverProgressObserver, SolverRunResult,
    SolverSolution, EXACT_MAX_LOCATIONS, TIME_SLOT_MINUTES,
};
use super::{
    frontier::{FrontierPolicy, ParetoState, TimeCostFrontierPolicy},
    stats::{ExactSolveResult, ExactSolverStats},
    transition::{initial_service, minutes_to_slot_ceil, slots_per_day},
};

/// Exact bitmask-DP solver for a single day and at most 15 locations.
///
/// The first and last locations keep the existing troute endpoint contract;
/// only intermediate locations are reordered. All routing data is supplied as
/// a matrix in `SolverInput`.
#[derive(Debug, Clone)]
pub struct ExactBitDpSolver<O = DefaultObjectivePolicy, F = TimeCostFrontierPolicy> {
    pub(in crate::solver) objective: O,
    frontier_policy: F,
}

impl Default for ExactBitDpSolver {
    fn default() -> Self {
        Self {
            objective: DefaultObjectivePolicy,
            frontier_policy: TimeCostFrontierPolicy,
        }
    }
}

impl<O, F> ExactBitDpSolver<O, F> {
    pub fn new(objective: O, frontier_policy: F) -> Self {
        Self {
            objective,
            frontier_policy,
        }
    }
}

impl<O: ObjectivePolicy, F: FrontierPolicy> ExactBitDpSolver<O, F> {
    pub fn solve_detailed(&self, input: SolverInput<'_>) -> Result<ExactSolveResult, SolverError> {
        self.validate(&input)?;
        let earliest_slot = minutes_to_slot_ceil(u32::from(input.problem.start_time().minutes()));
        if earliest_slot >= slots_per_day() {
            return Err(SolverError::NoFeasibleRoute);
        }

        let mut low = earliest_slot as u16;
        if matches!(
            input.problem.start_policy(),
            StartPolicy::Fixed | StartPolicy::Earliest
        ) {
            return self
                .search_at_start(&input, low)?
                .ok_or(SolverError::NoFeasibleRoute);
        }

        if input.problem.visit_policy() == crate::domain::VisitPolicy::BestEffortOnFailure {
            let target = self
                .search_at_start(&input, low)?
                .ok_or(SolverError::NoFeasibleRoute)?;
            let target_coverage = (
                target.metrics.visited_optional_count,
                target.metrics.visited_optional_priority,
            );
            let mut high = (slots_per_day() - 1) as u16;
            while low < high {
                let middle = low + (high - low).div_ceil(2);
                let preserves_target =
                    self.search_at_start(&input, middle)?.is_some_and(|result| {
                        (
                            result.metrics.visited_optional_count,
                            result.metrics.visited_optional_priority,
                        ) >= target_coverage
                    });
                if preserves_target {
                    low = middle;
                } else {
                    high = middle - 1;
                }
            }
            return self
                .search_at_start(&input, low)?
                .ok_or(SolverError::NoFeasibleRoute);
        }

        // Feasibility is monotone: an earlier departure can wait and reproduce
        // any later feasible route. Binary search avoids up to 1,440 full DPs.
        let mut high = (slots_per_day() - 1) as u16;
        if !self.is_feasible_at_start(&input, low)? {
            return Err(SolverError::NoFeasibleRoute);
        }
        while low < high {
            let middle = low + (high - low).div_ceil(2);
            if self.is_feasible_at_start(&input, middle)? {
                low = middle;
            } else {
                high = middle - 1;
            }
        }

        self.search_at_start(&input, low)?
            .ok_or(SolverError::NoFeasibleRoute)
    }

    fn validate(&self, input: &SolverInput<'_>) -> Result<(), SolverError> {
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let location_count = input.problem.locations().len();
        if location_count > EXACT_MAX_LOCATIONS {
            return Err(SolverError::UnsupportedLocationCount {
                maximum: EXACT_MAX_LOCATIONS,
                actual: location_count,
            });
        }
        if location_count == 0 {
            return Err(SolverError::NoFeasibleRoute);
        }
        if input.matrix.size() != location_count {
            return Err(SolverError::MatrixSizeMismatch {
                matrix: input.matrix.size(),
                locations: location_count,
            });
        }
        Ok(())
    }

    fn search_at_start(
        &self,
        input: &SolverInput<'_>,
        start_time_slot: u16,
    ) -> Result<Option<ExactSolveResult>, SolverError> {
        let Some(initial) = initial_service(input, start_time_slot) else {
            return Ok(None);
        };
        let location_count = input.problem.locations().len();
        let state_count = 1_usize << location_count;
        let full_mask = state_count - 1;
        let end = input.problem.end_location_index();
        let required_mask = input
            .problem
            .locations()
            .iter()
            .enumerate()
            .filter(|(_, location)| location.required())
            .fold(0_usize, |mask, (index, _)| mask | (1 << index));
        let partial =
            input.problem.visit_policy() == crate::domain::VisitPolicy::BestEffortOnFailure;
        let mut frontiers = vec![Vec::<usize>::new(); state_count * location_count];
        let mut arena = Vec::new();
        arena.push(ParetoState {
            time_slot: initial.departure_slot,
            travel_minutes: 0,
            wait_minutes: initial.wait_minutes,
            visited_optional_count: 0,
            visited_optional_priority: 0,
            score: self.objective.score(0, initial.wait_minutes),
            frontier_cost: self.objective.frontier_cost(0, initial.wait_minutes),
            predecessor: None,
            location: input.problem.start_location_index(),
        });
        frontiers[index(1, 0, location_count)].push(0);

        for mask in 1..=full_mask {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            for last in 0..location_count {
                if mask & (1 << last) == 0 {
                    continue;
                }
                let state_ids = frontiers[index(mask, last, location_count)].clone();
                for state_id in state_ids {
                    if last == end {
                        continue;
                    }
                    for next in 1..location_count {
                        let next_bit = 1 << next;
                        if mask & next_bit != 0
                            || !input
                                .problem
                                .allows_visit_at(next, mask.count_ones() as usize)
                        {
                            continue;
                        }
                        // The fixed destination can only be visited last.
                        if next == end {
                            let terminal_mask = mask | next_bit;
                            if (!partial && terminal_mask != full_mask)
                                || (terminal_mask & required_mask != required_mask)
                            {
                                continue;
                            }
                        }
                        let Some(candidate) =
                            self.transition(input, &arena[state_id], state_id, next)
                        else {
                            continue;
                        };
                        let new_mask = mask | next_bit;
                        let frontier = &mut frontiers[index(new_mask, next, location_count)];
                        if frontier.iter().any(|&existing_id| {
                            self.frontier_policy.dominates(
                                arena[existing_id].frontier_point(),
                                candidate.frontier_point(),
                            )
                        }) {
                            continue;
                        }
                        frontier.retain(|&existing_id| {
                            !self.frontier_policy.dominates(
                                candidate.frontier_point(),
                                arena[existing_id].frontier_point(),
                            )
                        });
                        let candidate_id = arena.len();
                        arena.push(candidate);
                        frontier.push(candidate_id);
                    }
                }
            }
        }

        let terminal_ids: Vec<_> = if partial {
            (1..=full_mask)
                .filter(|mask| mask & (1 << end) != 0 && mask & required_mask == required_mask)
                .flat_map(|mask| frontiers[index(mask, end, location_count)].iter().copied())
                .collect()
        } else {
            frontiers[index(full_mask, end, location_count)].clone()
        };
        let best = terminal_ids.into_iter().min_by(|&left, &right| {
            self.objective.compare(
                &metrics(input.problem.start_policy(), start_time_slot, &arena[left]),
                &metrics(input.problem.start_policy(), start_time_slot, &arena[right]),
            )
        });
        let Some(best_id) = best else {
            return Ok(None);
        };
        let mut visit_order = Vec::with_capacity(location_count);
        let mut cursor = Some(best_id);
        while let Some(state_id) = cursor {
            let state = &arena[state_id];
            visit_order.push(state.location);
            cursor = state.predecessor;
        }
        visit_order.reverse();
        Ok(Some(ExactSolveResult {
            solution: SolverSolution { visit_order },
            metrics: metrics(
                input.problem.start_policy(),
                start_time_slot,
                &arena[best_id],
            ),
            stats: ExactSolverStats {
                generated_states: arena.len(),
                frontier_states: frontiers.iter().map(Vec::len).sum(),
                frontier_cells: frontiers.iter().filter(|cell| !cell.is_empty()).count(),
            },
        }))
    }

    /// Feasibility needs only the earliest reachable time for each cell. This
    /// is used while locating the latest start slot so the full Pareto DP runs
    /// only once for the selected slot.
    fn is_feasible_at_start(
        &self,
        input: &SolverInput<'_>,
        start_time_slot: u16,
    ) -> Result<bool, SolverError> {
        super::find_feasible_at_start(input, start_time_slot, true)
            .map(|solution| solution.is_some())
    }

    fn transition(
        &self,
        input: &SolverInput<'_>,
        state: &ParetoState,
        predecessor: usize,
        next: usize,
    ) -> Option<ParetoState> {
        let travel_minutes = input.matrix.travel_minutes(state.location, next)?;
        let travel_slots = minutes_to_slot_ceil(travel_minutes);
        let arrival_slot = u32::from(state.time_slot).checked_add(travel_slots)?;
        let location = &input.problem.locations()[next];
        let open_slot = minutes_to_slot_ceil(u32::from(location.time_window().open().minutes()));
        let close_slot = u32::from(location.time_window().close().minutes()) / TIME_SLOT_MINUTES;
        let service_start = arrival_slot.max(open_slot);
        let finish_slot =
            service_start.checked_add(minutes_to_slot_ceil(location.stay_minutes()))?;
        if finish_slot > close_slot || finish_slot >= slots_per_day() {
            return None;
        }
        let wait_minutes = (service_start - arrival_slot).checked_mul(TIME_SLOT_MINUTES)?;
        let total_travel = state.travel_minutes.checked_add(travel_minutes)?;
        let total_wait = state.wait_minutes.checked_add(wait_minutes)?;
        let optional = !location.required();
        Some(ParetoState {
            time_slot: finish_slot as u16,
            travel_minutes: total_travel,
            wait_minutes: total_wait,
            visited_optional_count: state.visited_optional_count + usize::from(optional),
            visited_optional_priority: state.visited_optional_priority.checked_add(
                if optional {
                    u32::from(location.priority())
                } else {
                    0
                },
            )?,
            score: self.objective.score(total_travel, total_wait),
            frontier_cost: self.objective.frontier_cost(total_travel, total_wait),
            predecessor: Some(predecessor),
            location: next,
        })
    }
}

impl<O: ObjectivePolicy, F: FrontierPolicy> RouteSolver for ExactBitDpSolver<O, F> {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        self.solve_detailed(input).map(|result| result.solution)
    }

    fn solve_with_diagnostics_and_progress(
        &self,
        input: SolverInput<'_>,
        observer: &dyn SolverProgressObserver,
    ) -> Result<SolverRunResult, SolverError> {
        observer.on_progress(SolverProgressEvent::StrategyStarted(
            "exact_bit_dp".to_owned(),
        ));
        let result = self.solve_detailed(input);
        observer.on_progress(SolverProgressEvent::StrategyCompleted(
            "exact_bit_dp".to_owned(),
        ));
        result.map(|result| {
            observer.on_progress(SolverProgressEvent::SelectionStarted(1));
            SolverRunResult {
                solution: result.solution,
                diagnostics: None,
            }
        })
    }

    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        let selected = evaluate_solution(&self.objective, input, solution)?;
        super::super::selected_start_time(input.problem, &selected)
    }
}

fn metrics(
    start_policy: StartPolicy,
    start_time_slot: u16,
    state: &ParetoState,
) -> SolutionMetrics {
    SolutionMetrics {
        start_policy,
        visited_optional_count: state.visited_optional_count,
        visited_optional_priority: state.visited_optional_priority,
        start_time_slot,
        finish_time_slot: state.time_slot,
        travel_minutes: state.travel_minutes,
        wait_minutes: state.wait_minutes,
        score: state.score,
    }
}

fn index(mask: usize, last: usize, location_count: usize) -> usize {
    mask * location_count + last
}

#[cfg(test)]
mod tests;
