use super::*;

#[test]
fn seeded_shuffle_is_deterministic_and_preserves_endpoints() {
    let solution = SolverSolution {
        visit_order: vec![0, 1, 2, 3, 4],
    };

    let first = shuffle_solution(&solution, Some(1_234));
    let second = shuffle_solution(&solution, Some(1_234));

    assert_eq!(first, second);
    assert_ne!(first.visit_order, solution.visit_order);
    assert_eq!(first.visit_order.first(), Some(&0));
    assert_eq!(first.visit_order.last(), Some(&4));
    let mut intermediates = first.visit_order[1..4].to_vec();
    intermediates.sort_unstable();
    assert_eq!(intermediates, vec![1, 2, 3]);
}

#[test]
fn routes_without_two_intermediates_are_unchanged() {
    for visit_order in [vec![0, 1], vec![0, 1, 2]] {
        let solution = SolverSolution { visit_order };
        assert_eq!(shuffle_solution(&solution, Some(7)), solution);
    }
}
