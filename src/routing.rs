use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::{domain::Location, matrix::TravelTimeMatrix};

/// Options which affect a routing lookup but are independent of optimization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingContext {
    pub departure_time: Option<DateTime<Utc>>,
    pub travel_mode: TravelMode,
    pub timezone: Option<String>,
    pub options: BTreeMap<String, String>,
}

impl Default for RoutingContext {
    fn default() -> Self {
        Self {
            departure_time: None,
            travel_mode: TravelMode::Transit,
            timezone: None,
            options: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TravelMode {
    Transit,
    Driving,
    Walking,
    Bicycling,
}

impl TravelMode {
    pub fn as_provider_value(self) -> &'static str {
        match self {
            Self::Transit => "TRANSIT",
            Self::Driving => "DRIVING",
            Self::Walking => "WALKING",
            Self::Bicycling => "BICYCLING",
        }
    }
}

/// Supplies a complete directed matrix independently of the solver.
/// Providers with a native matrix API should implement this trait directly.
pub trait RoutingProvider {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError>;
}

impl<T> RoutingProvider for Box<T>
where
    T: RoutingProvider + ?Sized,
{
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        (**self).travel_time_matrix(locations, context)
    }
}

/// Optional pair-query boundary for providers without a native matrix API.
pub trait TravelTimeProvider {
    fn travel_time(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
    ) -> Result<TravelTime, RoutingError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TravelTime {
    pub minutes: u32,
}

/// Adapts a pair-query provider into the matrix boundary used by solvers.
#[derive(Debug, Clone)]
pub struct PairwiseMatrixRoutingProvider<P> {
    provider: P,
}

impl<P> PairwiseMatrixRoutingProvider<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }

    pub fn inner(&self) -> &P {
        &self.provider
    }
}

impl<P: TravelTimeProvider> RoutingProvider for PairwiseMatrixRoutingProvider<P> {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let mut rows = vec![vec![0; locations.len()]; locations.len()];
        for (from_index, from) in locations.iter().enumerate() {
            for (to_index, to) in locations.iter().enumerate() {
                if from_index != to_index {
                    rows[from_index][to_index] =
                        self.provider.travel_time(from, to, context)?.minutes;
                }
            }
        }
        TravelTimeMatrix::new(rows).map_err(RoutingError::InvalidMatrix)
    }
}

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

#[derive(Debug, Error)]
pub enum RoutingError {
    #[error("routing provider failed: {0}")]
    Provider(String),
    #[error("routing matrix size {matrix} does not match {locations} locations")]
    LocationCountMismatch { matrix: usize, locations: usize },
    #[error("routing provider produced an invalid matrix: {0}")]
    InvalidMatrix(#[source] crate::matrix::MatrixError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{RoutingReference, TimeOfDay, TimeWindow};

    fn locations() -> Vec<Location> {
        (0..3)
            .map(|index| {
                Location::new(
                    index.to_string(),
                    RoutingReference::GooglePlaceId(index.to_string()),
                    TimeWindow::new(
                        TimeOfDay::from_minutes(0).unwrap(),
                        TimeOfDay::from_minutes(1439).unwrap(),
                    )
                    .unwrap(),
                    0,
                )
            })
            .collect()
    }

    struct DirectedPairs;

    impl TravelTimeProvider for DirectedPairs {
        fn travel_time(
            &self,
            from: &Location,
            to: &Location,
            _context: &RoutingContext,
        ) -> Result<TravelTime, RoutingError> {
            Ok(TravelTime {
                minutes: from.id().parse::<u32>().unwrap() * 10 + to.id().parse::<u32>().unwrap(),
            })
        }
    }

    #[test]
    fn pair_queries_build_a_directed_matrix() {
        let matrix = PairwiseMatrixRoutingProvider::new(DirectedPairs)
            .travel_time_matrix(&locations(), &RoutingContext::default())
            .unwrap();
        assert_eq!(matrix.travel_minutes(0, 2), Some(2));
        assert_eq!(matrix.travel_minutes(2, 0), Some(20));
        assert_eq!(matrix.travel_minutes(1, 1), Some(0));
    }

    #[test]
    fn static_provider_validates_location_count() {
        let provider = StaticMatrixRoutingProvider::new(
            TravelTimeMatrix::new(vec![vec![0, 1], vec![2, 0]]).unwrap(),
        );
        assert!(matches!(
            provider.travel_time_matrix(&locations(), &RoutingContext::default()),
            Err(RoutingError::LocationCountMismatch { .. })
        ));
    }
}
