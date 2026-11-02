use std::{error::Error, num::NonZeroUsize, time::Duration};

use troute::{
    domain::Location,
    matrix::TravelTimeMatrix,
    providers::tcache::TcacheTravelTimeProvider,
    routing::{
        PairwiseMatrixRoutingProvider, ProviderSelectingRoutingProvider,
        RouteProviderSelectionDiagnostics, RoutingContext, RoutingError, RoutingProvider,
    },
};

use crate::config::RoutingConfig;

type ProductionRoutingProvider =
    ProviderSelectingRoutingProvider<Box<dyn RoutingProvider + Send + Sync>>;

struct UnconfiguredRoutingProvider;

impl RoutingProvider for UnconfiguredRoutingProvider {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let provider = context
            .options
            .get("routeProvider")
            .cloned()
            .unwrap_or_else(|| "tcache".to_owned());
        Err(RoutingError::ProviderNotConfigured {
            provider,
            reason: "TCACHE_BASE_URL is not configured; supply travel_time_matrix or sparse_travel_time_matrix"
                .to_owned(),
        })
    }
}

/// Production routing provider together with the diagnostics needed during
/// startup. Keeping these values together prevents the provider construction
/// contract from leaking as a positional tuple.
pub(super) struct RoutingRuntime {
    provider: ProductionRoutingProvider,
    provider_diagnostics: RouteProviderSelectionDiagnostics,
    pair_concurrency: NonZeroUsize,
    matrix_total_timeout: Duration,
    tcache_configured: bool,
}

impl RoutingRuntime {
    pub(super) fn initialize(config: RoutingConfig) -> Result<Self, Box<dyn Error>> {
        let RoutingConfig {
            tcache,
            provider_selector,
            pair_concurrency,
            matrix_total_timeout,
        } = config;

        let tcache_configured = tcache.is_some();
        let routing_provider: Box<dyn RoutingProvider + Send + Sync> = match tcache {
            Some(tcache) => {
                // reqwest's blocking client owns a small internal runtime.
                // Constructing it directly inside Tokio's async context panics
                // when that runtime is dropped, so perform the one-time startup
                // work in a blocking region.
                let travel_time_provider =
                    tokio::task::block_in_place(|| TcacheTravelTimeProvider::new(tcache))?;
                Box::new(PairwiseMatrixRoutingProvider::with_limits(
                    travel_time_provider,
                    pair_concurrency,
                    matrix_total_timeout,
                ))
            }
            None => Box::new(UnconfiguredRoutingProvider),
        };
        let provider_diagnostics = provider_selector.diagnostics();
        let provider = ProviderSelectingRoutingProvider::new(routing_provider, provider_selector);

        Ok(Self {
            provider,
            provider_diagnostics,
            pair_concurrency,
            matrix_total_timeout,
            tcache_configured,
        })
    }

    pub(super) fn provider_diagnostics(&self) -> &RouteProviderSelectionDiagnostics {
        &self.provider_diagnostics
    }

    pub(super) fn pair_concurrency(&self) -> NonZeroUsize {
        self.pair_concurrency
    }

    pub(super) fn matrix_total_timeout(&self) -> Duration {
        self.matrix_total_timeout
    }

    pub(super) fn tcache_configured(&self) -> bool {
        self.tcache_configured
    }

    pub(super) fn into_provider(self) -> ProductionRoutingProvider {
        self.provider
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_provider_reports_how_to_run_without_tcache() {
        let mut context = RoutingContext::default();
        context
            .options
            .insert("routeProvider".to_owned(), "google".to_owned());

        let error = UnconfiguredRoutingProvider
            .travel_time_matrix(&[], &context)
            .unwrap_err();

        assert!(matches!(
            error,
            RoutingError::ProviderNotConfigured { ref provider, ref reason }
                if provider == "google"
                    && reason.contains("supply travel_time_matrix or sparse_travel_time_matrix")
        ));
    }
}
