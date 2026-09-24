use std::time::Instant;

use thiserror::Error;

use crate::{cancellation::CancellationToken, domain::Location, matrix::TravelTimeMatrix};

use super::RoutingContext;

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

    /// Cooperatively cancels a pair lookup when another lookup in the matrix fails.
    /// Providers that can cancel in-flight work should override this method.
    fn travel_time_with_cancellation(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
        cancellation: &CancellationToken,
    ) -> Result<TravelTime, RoutingError> {
        if cancellation.is_cancelled() {
            return Err(RoutingError::Provider(
                "pair query cancelled before it started".to_owned(),
            ));
        }
        self.travel_time(from, to, context)
    }

    /// Applies a matrix-level deadline in addition to provider-specific pair limits.
    fn travel_time_with_cancellation_until(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
        cancellation: &CancellationToken,
        deadline: Instant,
    ) -> Result<TravelTime, RoutingError> {
        if Instant::now() >= deadline {
            return Err(RoutingError::Provider(
                "matrix deadline reached before pair query started".to_owned(),
            ));
        }
        self.travel_time_with_cancellation(from, to, context, cancellation)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TravelTime {
    pub minutes: u32,
}

#[derive(Debug, Error)]
pub enum RoutingError {
    #[error("routing provider failed: {0}")]
    Provider(String),
    #[error(
        "routing matrix failed: matrix_timeout={matrix_timeout}; elapsed_ms={elapsed_ms}; completed_pairs={completed_pairs}/{total_pairs}; failed_pair={failed_from} -> {failed_to}; cause={source}"
    )]
    MatrixBuild {
        matrix_timeout: bool,
        elapsed_ms: u64,
        completed_pairs: usize,
        total_pairs: usize,
        failed_from: String,
        failed_to: String,
        #[source]
        source: Box<RoutingError>,
    },
    #[error("routing matrix size {matrix} does not match {locations} locations")]
    LocationCountMismatch { matrix: usize, locations: usize },
    #[error("routing provider produced an invalid matrix: {0}")]
    InvalidMatrix(#[source] crate::matrix::MatrixError),
}
