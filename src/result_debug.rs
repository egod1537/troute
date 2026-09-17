use rand::{rngs::StdRng, seq::SliceRandom, Rng, SeedableRng};

use crate::solver::SolverSolution;

/// Returns a debug-only visit order with fixed endpoints and shuffled intermediates.
///
/// When at least two intermediate locations exist, the returned order is
/// guaranteed to differ from the solver order. Rejection sampling keeps every
/// non-identity permutation equally likely.
pub(crate) fn shuffle_solution(solution: &SolverSolution, seed: Option<u64>) -> SolverSolution {
    match seed {
        Some(seed) => shuffle_solution_with_rng(solution, &mut StdRng::seed_from_u64(seed)),
        None => shuffle_solution_with_rng(solution, &mut rand::thread_rng()),
    }
}

fn shuffle_solution_with_rng<R>(solution: &SolverSolution, rng: &mut R) -> SolverSolution
where
    R: Rng + ?Sized,
{
    let mut visit_order = solution.visit_order.clone();
    if visit_order.len() <= 3 {
        return SolverSolution { visit_order };
    }

    let intermediates = &mut visit_order[1..solution.visit_order.len() - 1];
    let original = intermediates.to_vec();
    while intermediates == original {
        intermediates.shuffle(rng);
    }

    SolverSolution { visit_order }
}

#[cfg(test)]
mod tests {
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
}
