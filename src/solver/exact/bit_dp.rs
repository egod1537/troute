use crate::domain::TimeOfDay;

use super::super::{
    evaluate_solution, DefaultObjectivePolicy, ObjectivePolicy, RouteSolver, SolutionMetrics,
    SolverError, SolverInput, SolverSolution, EXACT_MAX_LOCATIONS, TIME_SLOT_MINUTES,
};
use super::{
    frontier::{FrontierPolicy, ParetoState, TimeCostFrontierPolicy},
    stats::{ExactSolveResult, ExactSolverStats},
    transition::{minutes_to_slot_ceil, slots_per_day, transition_time},
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

        // Feasibility is monotone: an earlier departure can wait and reproduce
        // any later feasible route. Binary search avoids up to 144 full DPs.
        let mut low = earliest_slot as u16;
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
        let location_count = input.problem.locations().len();
        let state_count = 1_usize << location_count;
        let full_mask = state_count - 1;
        let end = input.problem.end_location_index();
        let mut frontiers = vec![Vec::<usize>::new(); state_count * location_count];
        let mut arena = Vec::new();
        arena.push(ParetoState {
            time_slot: start_time_slot,
            travel_minutes: 0,
            wait_minutes: 0,
            score: self.objective.score(0, 0),
            frontier_cost: self.objective.frontier_cost(0, 0),
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
                    for next in 1..location_count {
                        let next_bit = 1 << next;
                        if mask & next_bit != 0 {
                            continue;
                        }
                        // The fixed destination can only be visited last.
                        if next == end && mask | next_bit != full_mask {
                            continue;
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

        let terminal_ids = &frontiers[index(full_mask, end, location_count)];
        let best = terminal_ids.iter().copied().min_by(|&left, &right| {
            self.objective.compare(
                &metrics(start_time_slot, &arena[left]),
                &metrics(start_time_slot, &arena[right]),
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
            metrics: metrics(start_time_slot, &arena[best_id]),
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
        let location_count = input.problem.locations().len();
        if location_count == 1 {
            return Ok(true);
        }
        let state_count = 1_usize << location_count;
        let full_mask = state_count - 1;
        let end = input.problem.end_location_index();
        let mut earliest = vec![None::<u16>; state_count * location_count];
        earliest[index(1, 0, location_count)] = Some(start_time_slot);

        for mask in 1..=full_mask {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            for last in 0..location_count {
                let Some(time_slot) = earliest[index(mask, last, location_count)] else {
                    continue;
                };
                for next in 1..location_count {
                    let next_bit = 1 << next;
                    if mask & next_bit != 0 || (next == end && mask | next_bit != full_mask) {
                        continue;
                    }
                    let Some(finish_slot) = transition_time(input, last, next, time_slot) else {
                        continue;
                    };
                    let cell = &mut earliest[index(mask | next_bit, next, location_count)];
                    let improves = match *cell {
                        Some(current) => finish_slot < current,
                        None => true,
                    };
                    if improves {
                        *cell = Some(finish_slot);
                    }
                }
            }
        }
        Ok(earliest[index(full_mask, end, location_count)].is_some())
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
        Some(ParetoState {
            time_slot: finish_slot as u16,
            travel_minutes: total_travel,
            wait_minutes: total_wait,
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

    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        let selected = evaluate_solution(&self.objective, input, solution)?;
        TimeOfDay::from_minutes(selected.start_time_slot * TIME_SLOT_MINUTES as u16)
            .map_err(|error| SolverError::Failed(error.to_string()))
    }
}

fn metrics(start_time_slot: u16, state: &ParetoState) -> SolutionMetrics {
    SolutionMetrics {
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
mod tests {
    use super::*;
    use crate::{
        api::OptimizeRouteRequest,
        cancellation::CancellationToken,
        domain::{Location, OptimizationProblem, RoutingReference, TimeOfDay, TimeWindow},
        matrix::TravelTimeMatrix,
    };

    fn problem(windows_and_stays: &[(&str, &str, u32)], start: &str) -> OptimizationProblem {
        let locations: Vec<_> = windows_and_stays
            .iter()
            .enumerate()
            .map(|(index, (open, close, stay))| {
                serde_json::json!({
                    "id": index.to_string(),
                    "place_id": format!("place-{index}"),
                    "open_time": open,
                    "close_time": close,
                    "stay_minutes": stay
                })
            })
            .collect();
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "exact-solver-test",
            "locations": locations,
            "start_time": start
        }))
        .unwrap();
        request.try_into().unwrap()
    }

    fn detailed(
        problem: &OptimizationProblem,
        rows: Vec<Vec<u32>>,
    ) -> Result<ExactSolveResult, SolverError> {
        let matrix = TravelTimeMatrix::new(rows).unwrap();
        ExactBitDpSolver::default().solve_detailed(SolverInput {
            matrix: &matrix,
            problem,
            cancellation: &CancellationToken::new(),
        })
    }

    #[test]
    fn solves_one_location() {
        let problem = OptimizationProblem::new(
            vec![Location::new(
                "0".to_owned(),
                RoutingReference::GooglePlaceId("place-0".to_owned()),
                TimeWindow::new(
                    TimeOfDay::from_minutes(0).unwrap(),
                    TimeOfDay::from_minutes(1439).unwrap(),
                )
                .unwrap(),
                0,
            )],
            TimeOfDay::from_minutes(540).unwrap(),
        );
        let result = detailed(&problem, vec![vec![0]]).unwrap();
        assert_eq!(result.solution.visit_order, vec![0]);
        assert_eq!(result.metrics.start_time_slot, 143);
        assert_eq!(result.metrics.finish_time_slot, 143);
    }

    #[test]
    fn solves_three_locations_and_respects_opening_and_closing() {
        let problem = problem(
            &[
                ("00:00", "23:50", 0),
                ("10:00", "10:30", 20),
                ("00:00", "23:50", 0),
            ],
            "09:00",
        );
        let result = detailed(
            &problem,
            vec![vec![0, 10, 90], vec![10, 0, 10], vec![90, 10, 0]],
        )
        .unwrap();
        assert_eq!(result.solution.visit_order, vec![0, 1, 2]);
        assert_eq!(result.metrics.start_time_slot, 60); // 10:00
        assert_eq!(result.metrics.finish_time_slot, 64); // 10:40 at destination
    }

    #[test]
    fn records_waiting_at_a_fixed_start_slot() {
        let problem = problem(
            &[
                ("00:00", "23:50", 0),
                ("10:00", "18:00", 10),
                ("00:00", "23:50", 0),
            ],
            "09:00",
        );
        let matrix = TravelTimeMatrix::new(vec![vec![0; 3]; 3]).unwrap();
        let cancellation = CancellationToken::new();
        let input = SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &cancellation,
        };
        let result = ExactBitDpSolver::default()
            .search_at_start(&input, 54)
            .unwrap()
            .unwrap();
        assert_eq!(result.metrics.wait_minutes, 60);
    }

    #[test]
    fn reports_infeasible_time_windows() {
        let problem = problem(
            &[
                ("00:00", "23:50", 0),
                ("09:00", "09:10", 20),
                ("00:00", "23:50", 0),
            ],
            "09:00",
        );
        assert!(matches!(
            detailed(&problem, vec![vec![0; 3]; 3]),
            Err(SolverError::NoFeasibleRoute)
        ));
    }

    #[test]
    fn directed_times_choose_the_only_feasible_order() {
        let problem = problem(
            &[
                ("00:00", "23:50", 0),
                ("09:00", "12:00", 0),
                ("09:00", "12:00", 0),
                ("00:00", "23:50", 0),
            ],
            "09:00",
        );
        let huge = 240;
        let result = detailed(
            &problem,
            vec![
                vec![0, 10, huge, 0],
                vec![huge, 0, 10, 10],
                vec![huge, huge, 0, 10],
                vec![0, 0, 0, 0],
            ],
        )
        .unwrap();
        assert_eq!(result.solution.visit_order, vec![0, 1, 2, 3]);
    }

    #[test]
    fn latest_start_precedes_cost_and_finish_breaks_ties() {
        let problem = problem(
            &[
                ("00:00", "23:50", 0),
                ("09:00", "12:00", 0),
                ("09:00", "12:00", 0),
                ("00:00", "23:50", 0),
            ],
            "09:00",
        );
        let rows = vec![
            vec![0, 10, 10, 0],
            vec![0, 0, 20, 10],
            vec![0, 20, 0, 40],
            vec![0, 0, 0, 0],
        ];
        let result = detailed(&problem, rows.clone()).unwrap();
        assert_eq!(result.metrics.start_time_slot, 69); // 11:30
        assert_eq!(result.solution.visit_order, vec![0, 2, 1, 3]);
        assert_eq!(result.metrics.finish_time_slot, 73); // 12:10 at destination

        let matrix = TravelTimeMatrix::new(rows).unwrap();
        let selected = ExactBitDpSolver::default()
            .selected_start_time(
                SolverInput {
                    matrix: &matrix,
                    problem: &problem,
                    cancellation: &CancellationToken::new(),
                },
                &result.solution,
            )
            .unwrap();
        assert_eq!(selected.to_string(), "11:30");
    }

    #[test]
    fn pareto_pruning_retains_the_optimal_predecessor_chain() {
        let problem = problem(&[("00:00", "23:50", 0); 5], "09:00");
        let mut rows = vec![vec![600; 5]; 5];
        for (from, to, minutes) in [(0, 1, 10), (1, 2, 10), (2, 3, 10), (3, 4, 10)] {
            rows[from][to] = minutes;
        }
        // A slower path reaches the same (mask, last=3) frontier cell.
        for (from, to, minutes) in [(0, 2, 40), (2, 1, 40), (1, 3, 40)] {
            rows[from][to] = minutes;
        }
        for (index, row) in rows.iter_mut().enumerate() {
            row[index] = 0;
        }
        let matrix = TravelTimeMatrix::new(rows).unwrap();
        let cancellation = CancellationToken::new();
        let result = ExactBitDpSolver::default()
            .search_at_start(
                &SolverInput {
                    matrix: &matrix,
                    problem: &problem,
                    cancellation: &cancellation,
                },
                54,
            )
            .unwrap()
            .unwrap();
        assert_eq!(result.solution.visit_order, vec![0, 1, 2, 3, 4]);
        assert_eq!(result.metrics.travel_minutes, 40);
    }

    #[test]
    fn rejects_more_than_the_exact_limit() {
        let entries = vec![("00:00", "23:50", 0); EXACT_MAX_LOCATIONS + 1];
        let problem = problem(&entries, "09:00");
        let size = entries.len();
        assert!(matches!(
            detailed(&problem, vec![vec![0; size]; size]),
            Err(SolverError::UnsupportedLocationCount {
                maximum: EXACT_MAX_LOCATIONS,
                actual
            }) if actual == size
        ));
    }
}
