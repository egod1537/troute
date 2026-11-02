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
mod tests;
