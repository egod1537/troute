use std::collections::BTreeSet;

use rand::RngCore;

use super::{validate_generator_input, InitialRouteGenerator};
use crate::solver::{SolverError, SolverInput, SolverSolution};

#[derive(Debug, Clone, Copy, Default)]
pub struct GreedyInitialRoute;

impl InitialRouteGenerator for GreedyInitialRoute {
    fn generate(
        &self,
        input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        validate_generator_input(&input)?;
        let end = input.problem.end_location_index();
        if end == 0 {
            return Ok(SolverSolution {
                visit_order: vec![0],
            });
        }
        let mut remaining: BTreeSet<_> = (1..end).collect();
        let mut visit_order = vec![input.problem.start_location_index()];
        let mut current = input.problem.start_location_index();
        while !remaining.is_empty() {
            let next = remaining
                .iter()
                .copied()
                .min_by_key(|&candidate| {
                    (
                        input
                            .matrix
                            .travel_minutes(current, candidate)
                            .unwrap_or(u32::MAX),
                        input.problem.locations()[candidate]
                            .time_window()
                            .close()
                            .minutes(),
                        candidate,
                    )
                })
                .ok_or_else(|| SolverError::Failed("greedy route failed".to_owned()))?;
            remaining.remove(&next);
            visit_order.push(next);
            current = next;
        }
        visit_order.push(end);
        Ok(SolverSolution { visit_order })
    }
}

#[cfg(test)]
mod tests {
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
}
