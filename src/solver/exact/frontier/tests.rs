use super::*;

#[test]
fn dominance_requires_reproducible_time_and_cost() {
    let policy = TimeCostFrontierPolicy;
    assert!(policy.dominates(
        FrontierPoint {
            time_slot: 600,
            cost: 20,
        },
        FrontierPoint {
            time_slot: 610,
            cost: 30,
        },
    ));
    assert!(!policy.dominates(
        FrontierPoint {
            time_slot: 600,
            cost: 21,
        },
        FrontierPoint {
            time_slot: 610,
            cost: 30,
        },
    ));
    assert!(!policy.dominates(
        FrontierPoint {
            time_slot: 611,
            cost: 0,
        },
        FrontierPoint {
            time_slot: 610,
            cost: u64::MAX,
        },
    ));
    assert!(policy.dominates(
        FrontierPoint {
            time_slot: 1_430,
            cost: 7,
        },
        FrontierPoint {
            time_slot: 1_430,
            cost: 7,
        },
    ));
}

#[test]
fn dominance_never_uses_saturating_overflow_as_equality() {
    assert!(!TimeCostFrontierPolicy.dominates(
        FrontierPoint {
            time_slot: 0,
            cost: u64::MAX,
        },
        FrontierPoint {
            time_slot: 1,
            cost: u64::MAX,
        },
    ));
}
