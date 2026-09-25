use crate::solver::{
    evaluate_solution, minutes_to_slot_ceil, slots_per_day, ObjectivePolicy, SolutionMetrics,
    SolverError, SolverInput, SolverSolution, TIME_SLOT_MINUTES,
};

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
    let start_location = input.problem.start_location();
    let start_service = earliest_slot.max(minutes_to_slot_ceil(u32::from(
        start_location.time_window().open().minutes(),
    )));
    let mut time_slot =
        start_service.saturating_add(minutes_to_slot_ceil(start_location.stay_minutes()));
    let mut travel_minutes = 0_u32;
    let mut wait_minutes = (start_service - earliest_slot).saturating_mul(TIME_SLOT_MINUTES);
    let start_close_slot =
        u32::from(start_location.time_window().close().minutes()) / TIME_SLOT_MINUTES;
    let mut late_minutes = time_slot
        .saturating_sub(start_close_slot)
        .saturating_mul(TIME_SLOT_MINUTES);
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
        .filter(|_| {
            matches!(
                input.problem.start_policy(),
                crate::domain::StartPolicy::Latest
            )
        })
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
