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
mod tests;
