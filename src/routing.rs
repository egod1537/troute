use thiserror::Error;

use crate::{domain::Location, matrix::TravelTimeMatrix};

/// Supplies travel times independently of the optimization algorithm.
///
/// A Google Maps adapter and an optional cache can implement this boundary
/// without changing solver code.
pub trait RoutingProvider {
    fn travel_time_matrix(&self, locations: &[Location]) -> Result<TravelTimeMatrix, RoutingError>;
}

#[derive(Debug, Error)]
pub enum RoutingError {
    #[error("routing provider failed: {0}")]
    Provider(String),
}
