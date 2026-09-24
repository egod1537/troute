use rand::RngCore;

use super::super::{SolverError, SolverInput, SolverSolution};

/// Produces a complete visit order without requiring it to be feasible.
/// Consumers must evaluate the returned route against the original directed
/// matrix and the normal schedule constraints.
pub trait InitialRouteGenerator: Send + Sync {
    fn generate(
        &self,
        input: SolverInput<'_>,
        rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError>;
}

pub(super) fn validate_generator_input(input: &SolverInput<'_>) -> Result<(), SolverError> {
    if input.cancellation.is_cancelled() {
        return Err(SolverError::Cancelled);
    }
    let locations = input.problem.locations().len();
    if input.matrix.size() != locations {
        return Err(SolverError::MatrixSizeMismatch {
            matrix: input.matrix.size(),
            locations,
        });
    }
    Ok(())
}

pub(super) fn clone_input<'a>(input: &SolverInput<'a>) -> SolverInput<'a> {
    SolverInput {
        matrix: input.matrix,
        problem: input.problem,
        cancellation: input.cancellation,
    }
}
