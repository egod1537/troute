use std::{error::Error, num::NonZeroUsize, time::Duration};

use troute::{
    providers::tcache::TcacheTravelTimeProvider,
    routing::{
        PairwiseMatrixRoutingProvider, ProviderSelectingRoutingProvider,
        RouteProviderSelectionDiagnostics,
    },
};

use crate::config::RoutingConfig;

type ProductionRoutingProvider =
    ProviderSelectingRoutingProvider<PairwiseMatrixRoutingProvider<TcacheTravelTimeProvider>>;

/// Production routing provider together with the diagnostics needed during
/// startup. Keeping these values together prevents the provider construction
/// contract from leaking as a positional tuple.
pub(super) struct RoutingRuntime {
    provider: ProductionRoutingProvider,
    provider_diagnostics: RouteProviderSelectionDiagnostics,
    pair_concurrency: NonZeroUsize,
    matrix_total_timeout: Duration,
}

impl RoutingRuntime {
    pub(super) fn initialize(config: RoutingConfig) -> Result<Self, Box<dyn Error>> {
        let RoutingConfig {
            tcache,
            provider_selector,
            pair_concurrency,
            matrix_total_timeout,
        } = config;

        // reqwest's blocking client owns a small internal runtime. Constructing
        // it directly inside Tokio's async context panics when that runtime is
        // dropped, so perform the one-time startup work in a blocking region.
        let travel_time_provider =
            tokio::task::block_in_place(|| TcacheTravelTimeProvider::new(tcache))?;
        let provider_diagnostics = provider_selector.diagnostics();
        let provider = ProviderSelectingRoutingProvider::new(
            PairwiseMatrixRoutingProvider::with_limits(
                travel_time_provider,
                pair_concurrency,
                matrix_total_timeout,
            ),
            provider_selector,
        );

        Ok(Self {
            provider,
            provider_diagnostics,
            pair_concurrency,
            matrix_total_timeout,
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

    pub(super) fn into_provider(self) -> ProductionRoutingProvider {
        self.provider
    }
}
