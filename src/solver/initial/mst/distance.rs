use crate::{matrix::TravelTimeMatrix, solver::SolverError};

/// Converts the directed travel-time matrix into the undirected weights used
/// only while building an MST.
pub trait SymmetricDistanceStrategy: Send + Sync {
    fn name(&self) -> &'static str {
        "custom"
    }

    fn symmetric_distance(
        &self,
        matrix: &TravelTimeMatrix,
        left: usize,
        right: usize,
    ) -> Result<u64, SolverError>;
}

fn directed_pair(
    matrix: &TravelTimeMatrix,
    left: usize,
    right: usize,
) -> Result<(u32, u32), SolverError> {
    let forward = matrix.travel_minutes(left, right).ok_or_else(|| {
        SolverError::InvalidConfiguration(format!(
            "symmetric distance index {left}->{right} is outside the matrix"
        ))
    })?;
    let reverse = matrix.travel_minutes(right, left).ok_or_else(|| {
        SolverError::InvalidConfiguration(format!(
            "symmetric distance index {right}->{left} is outside the matrix"
        ))
    })?;
    Ok((forward, reverse))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AverageSymmetricDistance;

impl SymmetricDistanceStrategy for AverageSymmetricDistance {
    fn name(&self) -> &'static str {
        "average_bidirectional"
    }

    fn symmetric_distance(
        &self,
        matrix: &TravelTimeMatrix,
        left: usize,
        right: usize,
    ) -> Result<u64, SolverError> {
        let (forward, reverse) = directed_pair(matrix, left, right)?;
        Ok((u64::from(forward) + u64::from(reverse)) / 2)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MinSymmetricDistance;

impl SymmetricDistanceStrategy for MinSymmetricDistance {
    fn name(&self) -> &'static str {
        "minimum_bidirectional"
    }

    fn symmetric_distance(
        &self,
        matrix: &TravelTimeMatrix,
        left: usize,
        right: usize,
    ) -> Result<u64, SolverError> {
        let (forward, reverse) = directed_pair(matrix, left, right)?;
        Ok(u64::from(forward.min(reverse)))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MaxSymmetricDistance;

impl SymmetricDistanceStrategy for MaxSymmetricDistance {
    fn name(&self) -> &'static str {
        "maximum_bidirectional"
    }

    fn symmetric_distance(
        &self,
        matrix: &TravelTimeMatrix,
        left: usize,
        right: usize,
    ) -> Result<u64, SolverError> {
        let (forward, reverse) = directed_pair(matrix, left, right)?;
        Ok(u64::from(forward.max(reverse)))
    }
}
