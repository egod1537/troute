use std::time::Instant;

use thiserror::Error;

use crate::{cancellation::CancellationToken, domain::Location, matrix::TravelTimeMatrix};

use super::{RouteProviderSelection, RouteProviderSelectionDiagnostics, RoutingContext};

pub trait RoutingProgressObserver: Send + Sync {
    fn pairs_completed(&self, completed: usize, total: usize);
}

#[derive(Debug, Default)]
pub struct NoopRoutingProgressObserver;

impl RoutingProgressObserver for NoopRoutingProgressObserver {
    fn pairs_completed(&self, _completed: usize, _total: usize) {}
}

/// Supplies a complete directed matrix independently of the solver.
/// Providers with a native matrix API should implement this trait directly.
pub trait RoutingProvider {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError>;

    /// Builds a matrix while reporting real pair completions when the provider
    /// can observe them. Native batch providers may keep the default.
    fn travel_time_matrix_with_progress(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        _observer: &dyn RoutingProgressObserver,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        self.travel_time_matrix(locations, context)
    }

    /// Builds a matrix under the caller's cancellation signal and optional
    /// absolute deadline. Existing providers retain source compatibility via
    /// this default; providers that fan out pair work should override it.
    fn travel_time_matrix_with_control(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        observer: &dyn RoutingProgressObserver,
        cancellation: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        if cancellation.is_cancelled() {
            return Err(RoutingError::Cancelled);
        }
        let result = match deadline {
            Some(deadline) => self.travel_time_matrix_until(locations, context, deadline),
            None => self.travel_time_matrix_with_progress(locations, context, observer),
        };
        if cancellation.is_cancelled() {
            Err(RoutingError::Cancelled)
        } else {
            result
        }
    }

    /// Builds a matrix without starting work after the caller's deadline.
    /// Providers with deadline-aware I/O should override this method.
    fn travel_time_matrix_until(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        deadline: Instant,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        if Instant::now() >= deadline {
            return Err(RoutingError::Provider(
                "routing deadline reached before matrix query started".to_owned(),
            ));
        }
        self.travel_time_matrix(locations, context)
    }

    /// Revalidates one selected route leg at its actual departure instant.
    ///
    /// Providers with a pair API may override this to avoid constructing a
    /// two-location matrix. The default deliberately reuses the same routing
    /// contract and therefore preserves provider policy and sparse-arc
    /// semantics.
    fn travel_time_for_leg_with_control(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
        cancellation: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<Option<u32>, RoutingError> {
        let matrix = self.travel_time_matrix_with_control(
            &[from.clone(), to.clone()],
            context,
            &NoopRoutingProgressObserver,
            cancellation,
            deadline,
        )?;
        Ok(matrix.travel_minutes(0, 1))
    }

    fn provider_selection(
        &self,
        _context: &RoutingContext,
    ) -> Result<Option<RouteProviderSelection>, RoutingError> {
        Ok(None)
    }

    fn provider_policy_diagnostics(&self) -> Option<RouteProviderSelectionDiagnostics> {
        None
    }
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

    fn travel_time_matrix_with_progress(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        observer: &dyn RoutingProgressObserver,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        (**self).travel_time_matrix_with_progress(locations, context, observer)
    }

    fn travel_time_matrix_until(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        deadline: Instant,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        (**self).travel_time_matrix_until(locations, context, deadline)
    }

    fn travel_time_matrix_with_control(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        observer: &dyn RoutingProgressObserver,
        cancellation: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        (**self).travel_time_matrix_with_control(
            locations,
            context,
            observer,
            cancellation,
            deadline,
        )
    }

    fn travel_time_for_leg_with_control(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
        cancellation: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<Option<u32>, RoutingError> {
        (**self).travel_time_for_leg_with_control(from, to, context, cancellation, deadline)
    }

    fn provider_selection(
        &self,
        context: &RoutingContext,
    ) -> Result<Option<RouteProviderSelection>, RoutingError> {
        (**self).provider_selection(context)
    }

    fn provider_policy_diagnostics(&self) -> Option<RouteProviderSelectionDiagnostics> {
        (**self).provider_policy_diagnostics()
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
    #[error("routing request was cancelled")]
    Cancelled,
    #[error("no route exists between {from} and {to}: {reason}")]
    Unreachable {
        from: String,
        to: String,
        reason: String,
    },
    #[error("routing provider failed: {0}")]
    Provider(String),
    #[error("route provider resolution failed: {0}")]
    ProviderResolution(String),
    #[error("request-level route provider override is disabled")]
    ProviderOverrideDisabled,
    #[error("route provider {provider} is not configured: {reason}")]
    ProviderNotConfigured { provider: String, reason: String },
    #[error("route provider {provider} does not support mode {mode}")]
    UnsupportedProviderCapability { provider: String, mode: String },
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
