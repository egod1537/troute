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
mod tests;
