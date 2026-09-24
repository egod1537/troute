use rand::{seq::SliceRandom, RngCore};

use super::{validate_generator_input, InitialRouteGenerator};
use crate::solver::{SolverError, SolverInput, SolverSolution};

#[derive(Debug, Clone, Copy, Default)]
pub struct RandomInitialRoute;

impl InitialRouteGenerator for RandomInitialRoute {
    fn generate(
        &self,
        input: SolverInput<'_>,
        rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        validate_generator_input(&input)?;
        let end = input.problem.end_location_index();
        if end == 0 {
            return Ok(SolverSolution {
                visit_order: vec![0],
            });
        }
        let mut intermediate: Vec<_> = (1..end).collect();
        intermediate.shuffle(rng);
        let mut visit_order = Vec::with_capacity(input.problem.locations().len());
        visit_order.push(input.problem.start_location_index());
        visit_order.extend(intermediate);
        visit_order.push(end);
        Ok(SolverSolution { visit_order })
    }
}

#[cfg(test)]
mod tests {
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
}
