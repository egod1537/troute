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

#[derive(Debug, Clone)]
pub(super) struct ParetoState {
    pub(super) time_slot: u16,
    pub(super) travel_minutes: u32,
    pub(super) wait_minutes: u32,
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
