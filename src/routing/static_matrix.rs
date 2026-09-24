use crate::{domain::Location, matrix::TravelTimeMatrix};

use super::{RoutingContext, RoutingError, RoutingProvider};

/// Returns a precomputed matrix. Useful for tests and caller-owned routing.
#[derive(Debug, Clone)]
pub struct StaticMatrixRoutingProvider {
    matrix: TravelTimeMatrix,
}

impl StaticMatrixRoutingProvider {
    pub fn new(matrix: TravelTimeMatrix) -> Self {
        Self { matrix }
    }
}

impl RoutingProvider for StaticMatrixRoutingProvider {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        _context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        if self.matrix.size() != locations.len() {
            return Err(RoutingError::LocationCountMismatch {
                matrix: self.matrix.size(),
                locations: locations.len(),
            });
        }
        Ok(self.matrix.clone())
    }
}
