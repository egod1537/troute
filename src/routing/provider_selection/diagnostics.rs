use serde::{Deserialize, Serialize};

use crate::routing::TravelMode;

use super::{
    policy::RouteProviderPolicy, resolver::RouteProviderSelector, types::RouteProviderName,
};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAvailabilityDiagnostic {
    pub provider: RouteProviderName,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub capabilities: Vec<TravelMode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRouteDiagnostic {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<TravelMode>,
    pub provider: RouteProviderName,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteProviderSelectionDiagnostics {
    pub route_provider_mode: String,
    pub override_enabled: bool,
    pub policy: RouteProviderPolicy,
    pub providers: Vec<ProviderAvailabilityDiagnostic>,
    pub routes: Vec<PolicyRouteDiagnostic>,
}

impl RouteProviderSelector {
    pub fn diagnostics(&self) -> RouteProviderSelectionDiagnostics {
        let providers = self
            .registry
            .providers
            .iter()
            .map(|(&provider, registration)| ProviderAvailabilityDiagnostic {
                provider,
                available: registration.available,
                reason: registration.unavailable_reason.clone(),
                capabilities: registration.capabilities.clone(),
            })
            .collect();
        let mut routes = Vec::new();
        for (country, policy) in &self.policy.countries {
            for (&mode, &provider) in &policy.modes {
                routes.push(self.route_diagnostic(Some(country.clone()), Some(mode), provider));
            }
            if let Some(provider) = policy.default_provider {
                routes.push(self.route_diagnostic(Some(country.clone()), None, provider));
            }
        }
        for (&mode, &provider) in &self.policy.mode_defaults {
            routes.push(self.route_diagnostic(None, Some(mode), provider));
        }
        if let Some(provider) = self.policy.default_provider {
            routes.push(self.route_diagnostic(None, None, provider));
        }
        RouteProviderSelectionDiagnostics {
            route_provider_mode: self.mode.as_str().to_owned(),
            override_enabled: self.override_enabled,
            policy: self.policy.clone(),
            providers,
            routes,
        }
    }

    fn route_diagnostic(
        &self,
        country: Option<String>,
        mode: Option<TravelMode>,
        provider: RouteProviderName,
    ) -> PolicyRouteDiagnostic {
        let registration = self.registry.providers.get(&provider);
        let capability_supported = mode.is_none_or(|mode| {
            registration.is_some_and(|value| value.capabilities.contains(&mode))
        });
        let available = registration.is_some_and(|value| value.available) && capability_supported;
        let reason = if registration.is_none() {
            Some("provider is not registered".to_owned())
        } else if !registration.is_some_and(|value| value.available) {
            registration.and_then(|value| value.unavailable_reason.clone())
        } else if !capability_supported {
            Some("provider does not support this mode".to_owned())
        } else {
            None
        };
        PolicyRouteDiagnostic {
            country,
            mode,
            provider,
            available,
            reason,
        }
    }
}
