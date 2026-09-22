use std::{cmp::Ordering, time::Duration};

use thiserror::Error;

use crate::{
    cancellation::CancellationToken,
    domain::{OptimizationProblem, TimeOfDay},
    matrix::TravelTimeMatrix,
};

pub struct SolverInput<'a> {
    pub matrix: &'a TravelTimeMatrix,
    pub problem: &'a OptimizationProblem,
    pub cancellation: &'a CancellationToken,
}

/// The visit order uses every location index exactly once. It starts at index
/// zero, ends at the final index, and may reorder only intermediate indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverSolution {
    pub visit_order: Vec<usize>,
}

/// Finds a feasible order according to the implementation's objective policy.
pub trait RouteSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError>;

    /// Runs the solver and optionally returns strategy-level diagnostics.
    /// Implementations that do not orchestrate multiple strategies retain the
    /// original `solve` behavior through this default implementation.
    fn solve_with_diagnostics(
        &self,
        input: SolverInput<'_>,
    ) -> Result<SolverRunResult, SolverError> {
        self.solve(input).map(|solution| SolverRunResult {
            solution,
            diagnostics: None,
        })
    }

    /// Selects the schedule departure represented by the solution. Existing
    /// solvers retain the request start; exact solvers may choose a later slot.
    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        _solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        Ok(input.problem.start_time())
    }
}

pub const EXACT_MAX_LOCATIONS: usize = 15;
pub const TIME_SLOT_MINUTES: u32 = 10;

/// Values exposed to replaceable objective policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SolutionMetrics {
    pub start_time_slot: u16,
    pub finish_time_slot: u16,
    pub travel_minutes: u32,
    pub wait_minutes: u32,
    pub score: u64,
}

/// Common result returned by every orchestrated strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverCandidate {
    pub strategy: String,
    pub route: Option<SolverSolution>,
    pub feasible: bool,
    pub objective_score: Option<SolutionMetrics>,
    pub elapsed: Duration,
    pub metadata: SolverCandidateMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SolverCandidateMetadata {
    pub state_count: Option<usize>,
    pub frontier_state_count: Option<usize>,
    pub cluster_count: Option<usize>,
    pub iteration_count: Option<u64>,
    pub accepted_moves: Option<u64>,
    pub improved_moves: Option<u64>,
    pub seed: Option<u64>,
    pub timed_out: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverDiagnostics {
    pub selected_strategy: String,
    pub candidates: Vec<SolverCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverRunResult {
    pub solution: SolverSolution,
    pub diagnostics: Option<SolverDiagnostics>,
}

/// Keeps score construction and terminal ordering out of DP transitions.
pub trait ObjectivePolicy: Send + Sync {
    fn score(&self, travel_minutes: u32, wait_minutes: u32) -> u64;

    /// Scalar used only by the replaceable two-axis Pareto frontier. The
    /// default preserves legacy custom policies; the built-in policy encodes
    /// its travel-then-wait lexicographic order exactly.
    fn frontier_cost(&self, travel_minutes: u32, wait_minutes: u32) -> u64 {
        self.score(travel_minutes, wait_minutes)
    }

    /// `Ordering::Less` means that `left` is preferred.
    fn compare(&self, left: &SolutionMetrics, right: &SolutionMetrics) -> Ordering;
}

/// Evaluates and compares complete routes. The blanket implementation keeps
/// feasible-route scoring identical across exact and heuristic solvers.
pub trait ObjectiveEvaluator: Send + Sync {
    fn evaluate(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<SolutionMetrics, SolverError>;

    /// `Ordering::Less` means that `left` is preferred.
    fn compare(&self, left: &SolutionMetrics, right: &SolutionMetrics) -> Ordering;
}

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
        right
            .start_time_slot
            .cmp(&left.start_time_slot)
            .then_with(|| left.finish_time_slot.cmp(&right.finish_time_slot))
            .then_with(|| left.travel_minutes.cmp(&right.travel_minutes))
            .then_with(|| left.wait_minutes.cmp(&right.wait_minutes))
            .then_with(|| left.score.cmp(&right.score))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontierPoint {
    pub time_slot: u16,
    pub cost: u64,
}

/// Controls Pareto pruning independently of both transitions and objectives.
pub trait FrontierPolicy: Send + Sync {
    fn dominates(&self, left: FrontierPoint, right: FrontierPoint) -> bool;
}

/// The default frontier uses only time and cost. The delay adjustment preserves
/// a later state when an earlier state would have to incur extra waiting to
/// reproduce it.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeCostFrontierPolicy;

impl FrontierPolicy for TimeCostFrontierPolicy {
    fn dominates(&self, left: FrontierPoint, right: FrontierPoint) -> bool {
        if left.time_slot > right.time_slot {
            return false;
        }
        let delay_minutes =
            u64::from(right.time_slot - left.time_slot) * u64::from(TIME_SLOT_MINUTES);
        left.cost.saturating_add(delay_minutes) <= right.cost
    }
}

/// Exact result details are available without expanding the stable
/// `SolverSolution` hand-off used by the scheduler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactSolveResult {
    pub solution: SolverSolution,
    pub metrics: SolutionMetrics,
    pub stats: ExactSolverStats,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExactSolverStats {
    pub generated_states: usize,
    pub frontier_states: usize,
    pub frontier_cells: usize,
}

/// Exact bitmask-DP solver for a single day and at most 15 locations.
///
/// The first and last locations keep the existing troute endpoint contract;
/// only intermediate locations are reordered. All routing data is supplied as
/// a matrix in `SolverInput`.
#[derive(Debug, Clone)]
pub struct ExactBitDpSolver<O = DefaultObjectivePolicy, F = TimeCostFrontierPolicy> {
    objective: O,
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

#[derive(Debug, Clone)]
struct ParetoState {
    time_slot: u16,
    travel_minutes: u32,
    wait_minutes: u32,
    score: u64,
    frontier_cost: u64,
    predecessor: Option<usize>,
    location: usize,
}

impl ParetoState {
    fn frontier_point(&self) -> FrontierPoint {
        FrontierPoint {
            time_slot: self.time_slot,
            cost: self.frontier_cost,
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
    let mut low = earliest as u16;
    let mut high = (slots_per_day() - 1) as u16;
    if simulate_solution(objective, &input, solution, low).is_none() {
        return Err(SolverError::NoFeasibleRoute);
    }
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

/// Configurable weights for evaluating infeasible routes during heuristic
/// search. Feasible-route ordering remains owned by `ObjectivePolicy`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InfeasiblePenaltyWeights {
    pub late_minute_weight: f64,
    pub day_overflow_minute_weight: f64,
    pub required_location_violation_penalty: f64,
    pub latest_start_minute_reward: f64,
}

impl Default for InfeasiblePenaltyWeights {
    fn default() -> Self {
        Self {
            late_minute_weight: 100.0,
            day_overflow_minute_weight: 500.0,
            required_location_violation_penalty: 1_000_000.0,
            latest_start_minute_reward: 1.0,
        }
    }
}

impl InfeasiblePenaltyWeights {
    pub fn validate(self) -> Result<Self, SolverError> {
        for (name, value) in [
            ("late_minute_weight", self.late_minute_weight),
            (
                "day_overflow_minute_weight",
                self.day_overflow_minute_weight,
            ),
            (
                "required_location_violation_penalty",
                self.required_location_violation_penalty,
            ),
            (
                "latest_start_minute_reward",
                self.latest_start_minute_reward,
            ),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(SolverError::InvalidConfiguration(format!(
                    "{name} must be finite and non-negative"
                )));
            }
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PenalizedRouteEvaluation {
    pub energy: f64,
    pub feasible: bool,
    pub feasible_metrics: Option<SolutionMetrics>,
    pub travel_minutes: u32,
    pub wait_minutes: u32,
    pub late_minutes: u32,
    pub day_overflow_minutes: u32,
    pub required_location_violations: u32,
    pub penalty: f64,
}

/// Evaluates any route, including invalid or time-window-infeasible routes,
/// without turning ordinary constraint violations into hard rejection.
pub fn evaluate_solution_with_penalties(
    objective: &dyn ObjectivePolicy,
    input: SolverInput<'_>,
    solution: &SolverSolution,
    weights: InfeasiblePenaltyWeights,
) -> Result<PenalizedRouteEvaluation, SolverError> {
    let weights = weights.validate()?;
    if input.cancellation.is_cancelled() {
        return Err(SolverError::Cancelled);
    }
    let location_count = input.problem.locations().len();
    if input.matrix.size() != location_count {
        return Err(SolverError::MatrixSizeMismatch {
            matrix: input.matrix.size(),
            locations: location_count,
        });
    }

    let mut required_violations = solution.visit_order.len().abs_diff(location_count) as u32;
    let mut seen = vec![false; location_count];
    for &location in &solution.visit_order {
        if location >= location_count || seen[location] {
            required_violations = required_violations.saturating_add(1);
        } else {
            seen[location] = true;
        }
    }
    required_violations =
        required_violations.saturating_add(seen.iter().filter(|&&visited| !visited).count() as u32);
    if solution.visit_order.first() != Some(&input.problem.start_location_index()) {
        required_violations = required_violations.saturating_add(1);
    }
    if solution.visit_order.last() != Some(&input.problem.end_location_index()) {
        required_violations = required_violations.saturating_add(1);
    }

    let earliest_slot = minutes_to_slot_ceil(u32::from(input.problem.start_time().minutes()));
    let mut time_slot = earliest_slot;
    let mut travel_minutes = 0_u32;
    let mut wait_minutes = 0_u32;
    let mut late_minutes = 0_u32;
    let mut day_overflow_minutes = 0_u32;
    for edge in solution.visit_order.windows(2) {
        let (from, to) = (edge[0], edge[1]);
        if from >= location_count || to >= location_count {
            continue;
        }
        let travel = input
            .matrix
            .travel_minutes(from, to)
            .expect("matrix size was validated");
        travel_minutes = travel_minutes.saturating_add(travel);
        let arrival = time_slot.saturating_add(minutes_to_slot_ceil(travel));
        let location = &input.problem.locations()[to];
        let open_slot = minutes_to_slot_ceil(u32::from(location.time_window().open().minutes()));
        let service_start = arrival.max(open_slot);
        wait_minutes = wait_minutes
            .saturating_add((service_start - arrival).saturating_mul(TIME_SLOT_MINUTES));
        let finish = service_start.saturating_add(minutes_to_slot_ceil(location.stay_minutes()));
        let close_slot = u32::from(location.time_window().close().minutes()) / TIME_SLOT_MINUTES;
        late_minutes = late_minutes.saturating_add(
            finish
                .saturating_sub(close_slot)
                .saturating_mul(TIME_SLOT_MINUTES),
        );
        time_slot = finish;
    }
    if time_slot >= slots_per_day() {
        day_overflow_minutes = time_slot
            .saturating_sub(slots_per_day() - 1)
            .saturating_mul(TIME_SLOT_MINUTES);
    }

    let constraint_free =
        required_violations == 0 && late_minutes == 0 && day_overflow_minutes == 0;
    let candidate_metrics = if constraint_free {
        evaluate_solution(
            objective,
            SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: input.cancellation,
            },
            solution,
        )
        .ok()
    } else {
        None
    };
    let feasible = candidate_metrics.is_some();
    let base_score = candidate_metrics
        .map(|metrics| metrics.score)
        .unwrap_or_else(|| objective.score(travel_minutes, wait_minutes));
    let start_reward_minutes = candidate_metrics
        .map(|metrics| {
            u32::from(metrics.start_time_slot)
                .saturating_sub(earliest_slot)
                .saturating_mul(TIME_SLOT_MINUTES)
        })
        .unwrap_or(0);
    let penalty = f64::from(late_minutes) * weights.late_minute_weight
        + f64::from(day_overflow_minutes) * weights.day_overflow_minute_weight
        + f64::from(required_violations) * weights.required_location_violation_penalty;
    let energy = base_score as f64 + penalty
        - f64::from(start_reward_minutes) * weights.latest_start_minute_reward;

    Ok(PenalizedRouteEvaluation {
        energy,
        feasible,
        feasible_metrics: candidate_metrics,
        travel_minutes,
        wait_minutes,
        late_minutes,
        day_overflow_minutes,
        required_location_violations: required_violations,
        penalty,
    })
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
    let mut time_slot = u32::from(start);
    let mut travel_minutes = 0_u32;
    let mut wait_minutes = 0_u32;
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
        start_time_slot: start,
        finish_time_slot: time_slot as u16,
        travel_minutes,
        wait_minutes,
        score: objective.score(travel_minutes, wait_minutes),
    })
}

fn transition_time(input: &SolverInput<'_>, from: usize, to: usize, time_slot: u16) -> Option<u16> {
    let travel = input.matrix.travel_minutes(from, to)?;
    let location = &input.problem.locations()[to];
    let arrival = u32::from(time_slot).checked_add(minutes_to_slot_ceil(travel))?;
    let service_start = arrival.max(minutes_to_slot_ceil(u32::from(
        location.time_window().open().minutes(),
    )));
    let finish = service_start.checked_add(minutes_to_slot_ceil(location.stay_minutes()))?;
    let close = u32::from(location.time_window().close().minutes()) / TIME_SLOT_MINUTES;
    (finish <= close && finish < slots_per_day()).then_some(finish as u16)
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

fn minutes_to_slot_ceil(minutes: u32) -> u32 {
    minutes.div_ceil(TIME_SLOT_MINUTES)
}

fn slots_per_day() -> u32 {
    (24 * 60) / TIME_SLOT_MINUTES
}

fn index(mask: usize, last: usize, location_count: usize) -> usize {
    mask * location_count + last
}

mod clustered;
pub use clustered::*;
mod initial_route;
pub use initial_route::*;
mod christofides;
pub use christofides::*;
mod annealing;
pub use annealing::*;
mod orchestrator;
pub use orchestrator::*;

#[derive(Debug, Error)]
pub enum SolverError {
    #[error("solver was cancelled")]
    Cancelled,
    #[error("no feasible route was found")]
    NoFeasibleRoute,
    #[error("exact solver supports at most {maximum} locations (actual {actual})")]
    UnsupportedLocationCount { maximum: usize, actual: usize },
    #[error("bit-DP perfect matching supports at most {maximum} vertices (actual {actual})")]
    UnsupportedMatchingVertexCount { maximum: usize, actual: usize },
    #[error("minimum-weight perfect matching failed: {0}")]
    PerfectMatchingFailed(String),
    #[error("travel time matrix size {matrix} does not match {locations} locations")]
    MatrixSizeMismatch { matrix: usize, locations: usize },
    #[error("solver produced an invalid visit order")]
    InvalidVisitOrder,
    #[error("invalid solver configuration: {0}")]
    InvalidConfiguration(String),
    #[error("solver failed: {0}")]
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::OptimizeRouteRequest,
        domain::{Location, OptimizationProblem, RoutingReference, TimeOfDay, TimeWindow},
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
                ("00:00", "23:59", 0),
                ("10:00", "10:30", 20),
                ("00:00", "23:59", 0),
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
                ("00:00", "23:59", 0),
                ("10:00", "18:00", 10),
                ("00:00", "23:59", 0),
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
                ("00:00", "23:59", 0),
                ("09:00", "09:10", 20),
                ("00:00", "23:59", 0),
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
                ("00:00", "23:59", 0),
                ("09:00", "12:00", 0),
                ("09:00", "12:00", 0),
                ("00:00", "23:59", 0),
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
                ("00:00", "23:59", 0),
                ("09:00", "12:00", 0),
                ("09:00", "12:00", 0),
                ("00:00", "23:59", 0),
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
        let problem = problem(&[("00:00", "23:59", 0); 5], "09:00");
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
        let entries = vec![("00:00", "23:59", 0); EXACT_MAX_LOCATIONS + 1];
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
