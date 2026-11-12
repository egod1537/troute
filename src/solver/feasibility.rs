//! Bounded existence checks. A failed greedy attempt is never an infeasibility proof.
use rand::{rngs::StdRng, SeedableRng};
use serde::Serialize;

use super::{
    exact::find_feasible_at_start_until, GreedyInitialRoute, InitialRouteGenerator, SolverError,
    SolverInput, SolverSolution, EXACT_MAX_LOCATIONS,
};
use crate::schedule::calculate_schedule;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FeasibilityMethod {
    Greedy,
    BitDp,
}

#[derive(Debug, Clone)]
pub struct FeasibilitySolution {
    pub method: FeasibilityMethod,
    pub solution: SolverSolution,
}

pub fn check_route_feasibility(input: SolverInput<'_>) -> Result<FeasibilitySolution, SolverError> {
    check(input, None)
}

pub fn check_route_feasibility_until(
    input: SolverInput<'_>,
    deadline: std::time::Instant,
) -> Result<FeasibilitySolution, SolverError> {
    check(input, Some(deadline))
}

pub(super) fn check_active(
    input: &SolverInput<'_>,
    deadline: Option<std::time::Instant>,
) -> Result<(), SolverError> {
    if input.cancellation.is_cancelled() {
        return Err(SolverError::Cancelled);
    }
    if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
        return Err(SolverError::TimedOut);
    }
    Ok(())
}

fn check(
    input: SolverInput<'_>,
    deadline: Option<std::time::Instant>,
) -> Result<FeasibilitySolution, SolverError> {
    check_active(&input, deadline)?;
    let n = input.problem.locations().len();
    if n > EXACT_MAX_LOCATIONS {
        return Err(SolverError::UnsupportedLocationCount {
            maximum: EXACT_MAX_LOCATIONS,
            actual: n,
        });
    }
    if n == 0 {
        return Err(SolverError::InvalidVisitOrder);
    }
    if n != input.matrix.size() {
        return Err(SolverError::MatrixSizeMismatch {
            matrix: input.matrix.size(),
            locations: n,
        });
    }
    let mut rng = StdRng::seed_from_u64(0);
    if let Ok(solution) = GreedyInitialRoute.generate(input, &mut rng) {
        if calculate_schedule(input.problem, input.matrix, &solution).is_ok() {
            check_active(&input, deadline)?;
            return Ok(FeasibilitySolution {
                method: FeasibilityMethod::Greedy,
                solution,
            });
        }
    }
    // For EARLIEST/LATEST, existence only needs the minimum permitted start:
    // an earlier departure can wait and reproduce any later feasible schedule.
    let solution = find_feasible_at_start_until(
        &input,
        input.problem.start_time().minutes(),
        false,
        deadline,
    )?
    .ok_or(SolverError::ProvenInfeasible)?;
    calculate_schedule(input.problem, input.matrix, &solution)
        .map_err(|error| SolverError::Failed(error.to_string()))?;
    check_active(&input, deadline)?;
    Ok(FeasibilitySolution {
        method: FeasibilityMethod::BitDp,
        solution,
    })
}
