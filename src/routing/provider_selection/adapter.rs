use std::time::Instant;

use crate::{
    cancellation::CancellationToken,
    domain::Location,
    matrix::TravelTimeMatrix,
    routing::{RoutingContext, RoutingError, RoutingProgressObserver, RoutingProvider},
};

use super::{
    diagnostics::RouteProviderSelectionDiagnostics,
    resolver::RouteProviderSelector,
    types::{RouteProviderResolutionContext, RouteProviderSelection},
};

#[derive(Debug, Clone)]
pub struct ProviderSelectingRoutingProvider<P> {
    inner: P,
    selector: RouteProviderSelector,
}

impl<P> ProviderSelectingRoutingProvider<P> {
    pub fn new(inner: P, selector: RouteProviderSelector) -> Self {
        Self { inner, selector }
    }

    pub fn selector(&self) -> &RouteProviderSelector {
        &self.selector
    }

    pub fn resolver(&self) -> &RouteProviderSelector {
        self.selector()
    }
}

impl<P> ProviderSelectingRoutingProvider<P>
where
    P: RoutingProvider,
{
    fn resolved_context(&self, context: &RoutingContext) -> Result<RoutingContext, RoutingError> {
        let selection = self.provider_selection(context)?.ok_or_else(|| {
            RoutingError::ProviderResolution("provider policy returned no selection".to_owned())
        })?;
        let mut resolved = context.clone();
        resolved
            .options
            .insert("routeProvider".to_owned(), selection.provider.to_string());
        Ok(resolved)
    }
}

impl<P> RoutingProvider for ProviderSelectingRoutingProvider<P>
where
    P: RoutingProvider,
{
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        self.inner
            .travel_time_matrix(locations, &self.resolved_context(context)?)
    }

    fn travel_time_matrix_until(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        deadline: Instant,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        self.inner
            .travel_time_matrix_until(locations, &self.resolved_context(context)?, deadline)
    }

    fn travel_time_matrix_with_progress(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        observer: &dyn RoutingProgressObserver,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        self.inner.travel_time_matrix_with_progress(
            locations,
            &self.resolved_context(context)?,
            observer,
        )
    }

    fn travel_time_matrix_with_control(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        observer: &dyn RoutingProgressObserver,
        cancellation: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        self.inner.travel_time_matrix_with_control(
            locations,
            &self.resolved_context(context)?,
            observer,
            cancellation,
            deadline,
        )
    }

    fn provider_selection(
        &self,
        context: &RoutingContext,
    ) -> Result<Option<RouteProviderSelection>, RoutingError> {
        self.selector
            .resolve(&RouteProviderResolutionContext {
                country_code: context.options.get("countryCode").cloned(),
                mode: context.travel_mode,
                request_override: context
                    .options
                    .get("routeProviderOverride")
                    .map(|value| value.parse())
                    .transpose()
                    .map_err(RoutingError::ProviderResolution)?,
            })
            .map(Some)
    }

    fn provider_policy_diagnostics(&self) -> Option<RouteProviderSelectionDiagnostics> {
        Some(self.selector.diagnostics())
    }
}
