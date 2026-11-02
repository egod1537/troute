use rand::{rngs::StdRng, SeedableRng};

use super::*;
use crate::{cancellation::CancellationToken, matrix::TravelTimeMatrix};

#[test]
fn follows_directed_nearest_neighbors_and_preserves_endpoints() {
    let problem = super::super::test_support::problem(5);
    let matrix = TravelTimeMatrix::new(vec![
        vec![0, 30, 10, 20, 0],
        vec![30, 0, 30, 5, 0],
        vec![10, 5, 0, 15, 0],
        vec![20, 5, 15, 0, 0],
        vec![0, 0, 0, 0, 0],
    ])
    .unwrap();
    let cancellation = CancellationToken::new();
    let mut rng = StdRng::seed_from_u64(42);

    let solution = GreedyInitialRoute
        .generate(
            super::super::test_support::input(&problem, &matrix, &cancellation),
            &mut rng,
        )
        .unwrap();

    assert_eq!(solution.visit_order, vec![0, 2, 1, 3, 4]);
}
