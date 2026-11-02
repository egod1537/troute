mod adapter;
mod diagnostics;
mod policy;
mod registry;
mod resolver;
mod types;

pub use adapter::ProviderSelectingRoutingProvider;
pub use diagnostics::{
    PolicyRouteDiagnostic, ProviderAvailabilityDiagnostic, RouteProviderSelectionDiagnostics,
};
pub use policy::{CountryRouteProviderPolicy, RouteProviderPolicy};
pub use registry::RouteProviderRegistry;
pub use resolver::RouteProviderSelector;
pub use types::{
    normalize_country_code, RouteProviderMode, RouteProviderName, RouteProviderResolutionContext,
    RouteProviderSelection, RouteProviderSelectionSource,
};

// Public compatibility aliases. Internal code uses the selection terminology.
pub use ProviderSelectingRoutingProvider as PolicyRoutingProvider;
pub use RouteProviderSelectionDiagnostics as RouteProviderPolicyDiagnostics;
pub use RouteProviderSelector as RouteProviderPolicyResolver;

#[cfg(test)]
mod tests;
