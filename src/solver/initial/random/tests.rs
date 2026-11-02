use std::collections::BTreeSet;

use rand::{rngs::StdRng, SeedableRng};

use super::*;
use crate::{cancellation::CancellationToken, matrix::TravelTimeMatrix};

#[test]
fn seeded_shuffle_is_deterministic_and_preserves_endpoints() {
    let problem = super::super::test_support::problem(6);
    let matrix = TravelTimeMatrix::new(vec![vec![0; 6]; 6]).unwrap();
    let cancellation = CancellationToken::new();
    let mut first_rng = StdRng::seed_from_u64(42);
    let mut second_rng = StdRng::seed_from_u64(42);

    let first = RandomInitialRoute
        .generate(
            super::super::test_support::input(&problem, &matrix, &cancellation),
            &mut first_rng,
        )
        .unwrap();
    let second = RandomInitialRoute
        .generate(
            super::super::test_support::input(&problem, &matrix, &cancellation),
            &mut second_rng,
        )
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(first.visit_order.first(), Some(&0));
    assert_eq!(first.visit_order.last(), Some(&5));
    assert_eq!(
        first.visit_order.iter().copied().collect::<BTreeSet<_>>(),
        (0..6).collect()
    );
}
