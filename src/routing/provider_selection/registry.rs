use std::collections::BTreeMap;

use crate::routing::{RoutingError, TravelMode};

use super::types::RouteProviderName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProviderRegistration {
    pub(super) available: bool,
    pub(super) unavailable_reason: Option<String>,
    pub(super) capabilities: Vec<TravelMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteProviderRegistry {
    pub(super) providers: BTreeMap<RouteProviderName, ProviderRegistration>,
}

impl Default for RouteProviderRegistry {
    fn default() -> Self {
        use RouteProviderName as Provider;
        use TravelMode as Mode;
        let all = vec![Mode::Driving, Mode::Walking, Mode::Bicycling, Mode::Transit];
        Self {
            providers: BTreeMap::from([
                (Provider::Google, registration(all.clone())),
                (Provider::KakaoMobility, registration(vec![Mode::Driving])),
                (
                    Provider::KakaoMaps,
                    registration(vec![Mode::Walking, Mode::Bicycling, Mode::Transit]),
                ),
                (Provider::Ekispert, registration(vec![Mode::Transit])),
                (Provider::Navitime, registration(all)),
                (Provider::Otp, registration(vec![Mode::Transit])),
            ]),
        }
    }
}

fn registration(capabilities: Vec<TravelMode>) -> ProviderRegistration {
    ProviderRegistration {
        available: true,
        unavailable_reason: None,
        capabilities,
    }
}

impl RouteProviderRegistry {
    pub fn set_availability(
        &mut self,
        provider: RouteProviderName,
        available: bool,
        reason: Option<String>,
    ) {
        if let Some(registration) = self.providers.get_mut(&provider) {
            registration.available = available;
            registration.unavailable_reason = (!available).then_some(reason).flatten();
        }
    }

    pub(super) fn validate_selection(
        &self,
        provider: RouteProviderName,
        mode: TravelMode,
    ) -> Result<(), RoutingError> {
        let registration =
            self.providers
                .get(&provider)
                .ok_or_else(|| RoutingError::ProviderNotConfigured {
                    provider: provider.to_string(),
                    reason: "provider is not registered".to_owned(),
                })?;
        if !registration.available {
            return Err(RoutingError::ProviderNotConfigured {
                provider: provider.to_string(),
                reason: registration
                    .unavailable_reason
                    .clone()
                    .unwrap_or_else(|| "provider is unavailable".to_owned()),
            });
        }
        if !registration.capabilities.contains(&mode) {
            return Err(RoutingError::UnsupportedProviderCapability {
                provider: provider.to_string(),
                mode: mode.as_provider_value().to_owned(),
            });
        }
        Ok(())
    }
}
