use std::time::Instant;

use chrono::{DateTime, Utc};

use crate::{
    cancellation::CancellationToken,
    domain::Location,
    routing::{
        RouteProviderName, RouteProviderSelection, RouteProviderSelectionDiagnostics,
        RoutingContext, RoutingError, RoutingProgressObserver, RoutingProvider, TravelMode,
    },
};

use super::TravelTimeMatrix;

/// Selects the source used to construct a travel-time matrix.
pub enum MatrixSource {
    Provider,
    Dense(Vec<Vec<u32>>),
    Sparse(Vec<Vec<Option<u32>>>),
}

/// Owns the routing provider and provides the common matrix-construction boundary.
pub struct MatrixService<P> {
    provider: P,
}

impl<P> MatrixService<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

impl<P> MatrixService<P>
where
    P: RoutingProvider,
{
    pub fn routing_context(
        &self,
        departure_time: DateTime<Utc>,
        travel_mode: TravelMode,
        timezone: Option<String>,
        country_code: Option<&str>,
        provider: Option<RouteProviderName>,
    ) -> RoutingContext {
        let mut context = RoutingContext {
            departure_time: Some(departure_time),
            travel_mode,
            timezone,
            ..RoutingContext::default()
        };
        if let Some(country_code) = country_code {
            context
                .options
                .insert("countryCode".to_owned(), country_code.to_owned());
        }
        if let Some(provider) = provider {
            context
                .options
                .insert("routeProviderOverride".to_owned(), provider.to_string());
        }
        context
    }

    pub fn provider_selection(
        &self,
        context: &RoutingContext,
    ) -> Result<Option<RouteProviderSelection>, RoutingError> {
        self.provider.provider_selection(context)
    }

    pub fn provider_policy_diagnostics(&self) -> Option<RouteProviderSelectionDiagnostics> {
        self.provider.provider_policy_diagnostics()
    }

    pub fn build(
        &self,
        locations: &[Location],
        source: MatrixSource,
        context: &RoutingContext,
        observer: &dyn RoutingProgressObserver,
        cancellation: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        if cancellation.is_cancelled() {
            return Err(RoutingError::Cancelled);
        }
        let matrix = self.build_from_source(locations, source, || {
            self.provider.travel_time_matrix_with_control(
                locations,
                context,
                observer,
                cancellation,
                deadline,
            )
        })?;
        if cancellation.is_cancelled() {
            Err(RoutingError::Cancelled)
        } else {
            Ok(matrix)
        }
    }

    pub fn build_until(
        &self,
        locations: &[Location],
        source: MatrixSource,
        context: &RoutingContext,
        deadline: Instant,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        self.build_from_source(locations, source, || {
            self.provider
                .travel_time_matrix_until(locations, context, deadline)
        })
    }

    pub fn travel_time_for_leg_with_control(
        &self,
        from: &Location,
        to: &Location,
        context: &RoutingContext,
        cancellation: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<Option<u32>, RoutingError> {
        self.provider
            .travel_time_for_leg_with_control(from, to, context, cancellation, deadline)
    }

    fn build_from_source(
        &self,
        locations: &[Location],
        source: MatrixSource,
        build_from_provider: impl FnOnce() -> Result<TravelTimeMatrix, RoutingError>,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let (matrix, validate_location_count) = match source {
            MatrixSource::Provider => (build_from_provider()?, false),
            MatrixSource::Dense(rows) => (
                TravelTimeMatrix::new(rows).map_err(RoutingError::InvalidMatrix)?,
                true,
            ),
            MatrixSource::Sparse(rows) => (
                TravelTimeMatrix::new_sparse(rows).map_err(RoutingError::InvalidMatrix)?,
                true,
            ),
        };
        if validate_location_count && matrix.size() != locations.len() {
            return Err(RoutingError::LocationCountMismatch {
                matrix: matrix.size(),
                locations: locations.len(),
            });
        }
        Ok(matrix)
    }
}
