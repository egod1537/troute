use super::super::TIME_SLOT_MINUTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontierPoint {
    pub time_slot: u16,
    pub cost: u64,
}

/// Controls Pareto pruning independently of both transitions and objectives.
pub trait FrontierPolicy: Send + Sync {
    fn dominates(&self, left: FrontierPoint, right: FrontierPoint) -> bool;
}

/// A sufficient (deliberately conservative) dominance test for two states in
/// the same `(visited_mask, last_location)` cell.
///
/// If `left` finishes no later than `right`, it can idle at the already
/// serviced location for exactly the time difference and start every remaining
/// transition at the same minute as `right`. Time windows only constrain
/// service completion, so that idle step does not invalidate prior service.
/// `frontier_cost + delay` represents the cost after that reproduction. The
/// state is discarded only when the reproduced left cost is no worse.
///
/// The built-in cost packs `(travel_minutes, wait_minutes)` in lexicographic
/// order. A feasible single-day state's wait and the at-most-1,439-minute delay
/// cannot carry into the upper travel half. Custom policies may use the full
/// `u64`, so overflow intentionally means "not dominated".
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeCostFrontierPolicy;

impl FrontierPolicy for TimeCostFrontierPolicy {
    fn dominates(&self, left: FrontierPoint, right: FrontierPoint) -> bool {
        if left.time_slot > right.time_slot {
            return false;
        }
        let delay_minutes =
            u64::from(right.time_slot - left.time_slot) * u64::from(TIME_SLOT_MINUTES);
        left.cost
            .checked_add(delay_minutes)
            .is_some_and(|reproduced_cost| reproduced_cost <= right.cost)
    }
}

#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub(super) struct ParetoState {
    pub(super) time_slot: u16,
    pub(super) travel_minutes: u32,
    pub(super) wait_minutes: u32,
    pub(super) visited_optional_count: usize,
    pub(super) visited_optional_priority: u32,
    pub(super) score: u64,
    pub(super) frontier_cost: u64,
    pub(super) predecessor: Option<usize>,
    pub(super) location: usize,
}

impl ParetoState {
    pub(super) fn frontier_point(&self) -> FrontierPoint {
        FrontierPoint {
            time_slot: self.time_slot,
            cost: self.frontier_cost,
        }
    }
}
