use std::{collections::BTreeSet, env};

use crate::routing::{RoutingError, TravelMode};

use super::{
    policy::RouteProviderPolicy,
    registry::RouteProviderRegistry,
    types::{
        normalize_country_code, RouteProviderMode, RouteProviderName,
        RouteProviderResolutionContext, RouteProviderSelection, RouteProviderSelectionSource,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteProviderSelector {
    pub(super) mode: RouteProviderMode,
    pub(super) policy: RouteProviderPolicy,
    pub(super) override_enabled: bool,
    pub(super) registry: RouteProviderRegistry,
    pub(super) legacy_routes: BTreeSet<(String, TravelMode)>,
}

impl RouteProviderSelector {
    pub fn resolve(
        &self,
        context: &RouteProviderResolutionContext,
    ) -> Result<RouteProviderSelection, RoutingError> {
        if let RouteProviderMode::Force(provider) = self.mode {
            return self.finish(
                provider,
                context.mode,
                RouteProviderSelectionSource::GlobalForce,
                format!("ROUTE_PROVIDER forces {provider}"),
            );
        }
        if let Some(provider) = context.request_override {
            if !self.override_enabled {
                return Err(RoutingError::ProviderOverrideDisabled);
            }
            return self.finish(
                provider,
                context.mode,
                RouteProviderSelectionSource::RequestOverride,
                format!("request explicitly selected {provider}"),
            );
        }
        let country = context
            .country_code
            .as_deref()
            .map(normalize_country_code)
            .transpose()
            .map_err(RoutingError::ProviderResolution)?;
        if let Some(country) = country.as_deref() {
            if let Some(country_policy) = self.policy.countries.get(country) {
                if let Some(&provider) = country_policy.modes.get(&context.mode) {
                    let source = if self
                        .legacy_routes
                        .contains(&(country.to_owned(), context.mode))
                    {
                        RouteProviderSelectionSource::Legacy
                    } else {
                        RouteProviderSelectionSource::CountryMode
                    };
                    return self.finish(
                        provider,
                        context.mode,
                        source,
                        format!(
                            "country {country} and mode {} matched policy",
                            context.mode.as_provider_value()
                        ),
                    );
                }
                if let Some(provider) = country_policy.default_provider {
                    return self.finish(
                        provider,
                        context.mode,
                        RouteProviderSelectionSource::CountryDefault,
                        format!("country {country} matched its default provider"),
                    );
                }
            }
        }
        if let Some(&provider) = self.policy.mode_defaults.get(&context.mode) {
            return self.finish(
                provider,
                context.mode,
                RouteProviderSelectionSource::ModeDefault,
                format!(
                    "mode {} matched its default provider",
                    context.mode.as_provider_value()
                ),
            );
        }
        if let Some(provider) = self.policy.default_provider {
            return self.finish(
                provider,
                context.mode,
                RouteProviderSelectionSource::GlobalDefault,
                "global default provider matched".to_owned(),
            );
        }
        Err(RoutingError::ProviderResolution(format!(
            "no route provider policy matches country={} mode={}",
            country.as_deref().unwrap_or("<missing>"),
            context.mode.as_provider_value()
        )))
    }

    fn finish(
        &self,
        provider: RouteProviderName,
        mode: TravelMode,
        source: RouteProviderSelectionSource,
        reason: String,
    ) -> Result<RouteProviderSelection, RoutingError> {
        self.registry.validate_selection(provider, mode)?;
        Ok(RouteProviderSelection {
            provider,
            reason,
            source,
        })
    }

    pub fn from_env() -> Result<Self, String> {
        let registry = RouteProviderRegistry::default();
        let route_provider = env::var("ROUTE_PROVIDER").unwrap_or_else(|_| "auto".to_owned());
        let policy_json = env::var("ROUTE_PROVIDER_POLICY_JSON").ok();
        let legacy = env::var("JAPAN_TRANSIT_PROVIDER").ok();
        let override_enabled = read_optional_env("ROUTE_PROVIDER_OVERRIDE_ENABLED")?;
        Self::from_raw_config_sources(
            &route_provider,
            policy_json.as_deref(),
            legacy.as_deref(),
            override_enabled.as_deref(),
            registry,
        )
    }

    /// Parses the raw override flag and delegates policy validation to the
    /// typed constructor. Applications can use this without reimplementing
    /// the compatibility bool grammar.
    pub fn from_raw_config_sources(
        route_provider: &str,
        policy_json: Option<&str>,
        legacy_japan_transit_provider: Option<&str>,
        override_enabled: Option<&str>,
        registry: RouteProviderRegistry,
    ) -> Result<Self, String> {
        let override_enabled =
            parse_bool_value("ROUTE_PROVIDER_OVERRIDE_ENABLED", override_enabled, false)?;
        Self::from_config_sources(
            route_provider,
            policy_json,
            legacy_japan_transit_provider,
            override_enabled,
            registry,
        )
    }

    pub fn from_config_sources(
        route_provider: &str,
        policy_json: Option<&str>,
        legacy_japan_transit_provider: Option<&str>,
        override_enabled: bool,
        registry: RouteProviderRegistry,
    ) -> Result<Self, String> {
        let mode = if route_provider.trim().eq_ignore_ascii_case("auto") {
            RouteProviderMode::Auto
        } else {
            RouteProviderMode::Force(route_provider.parse()?)
        };
        let custom_policy = policy_json.is_some_and(|value| !value.trim().is_empty());
        let mut policy = match policy_json {
            Some(value) if !value.trim().is_empty() => serde_json::from_str(value)
                .map_err(|error| format!("ROUTE_PROVIDER_POLICY_JSON is invalid: {error}"))?,
            _ => RouteProviderPolicy::built_in(),
        };
        policy = policy.normalize()?;
        let mut legacy_routes = BTreeSet::new();
        if let Some(value) = legacy_japan_transit_provider {
            let provider: RouteProviderName = value
                .parse()
                .map_err(|error| format!("JAPAN_TRANSIT_PROVIDER is invalid: {error}"))?;
            let explicitly_configured = custom_policy
                && policy
                    .countries
                    .get("JP")
                    .is_some_and(|country| country.modes.contains_key(&TravelMode::Transit));
            if !explicitly_configured {
                policy
                    .countries
                    .entry("JP".to_owned())
                    .or_default()
                    .modes
                    .insert(TravelMode::Transit, provider);
                legacy_routes.insert(("JP".to_owned(), TravelMode::Transit));
            }
        }
        validate_policy_registry(&policy, &registry)?;
        if let RouteProviderMode::Force(provider) = mode {
            if !registry.providers.contains_key(&provider) {
                return Err(format!(
                    "ROUTE_PROVIDER references unregistered provider {provider}"
                ));
            }
        }
        Ok(Self {
            mode,
            policy,
            override_enabled,
            registry,
            legacy_routes,
        })
    }
}

fn validate_policy_registry(
    policy: &RouteProviderPolicy,
    registry: &RouteProviderRegistry,
) -> Result<(), String> {
    for provider in policy
        .countries
        .values()
        .flat_map(|country| {
            country
                .modes
                .values()
                .chain(country.default_provider.iter())
        })
        .chain(policy.mode_defaults.values())
        .chain(policy.default_provider.iter())
    {
        if !registry.providers.contains_key(provider) {
            return Err(format!(
                "policy references unregistered provider {provider}"
            ));
        }
    }
    Ok(())
}

fn read_optional_env(name: &str) -> Result<Option<String>, String> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn parse_bool_value(name: &str, value: Option<&str>, default: bool) -> Result<bool, String> {
    match value {
        Some(value) if value.eq_ignore_ascii_case("true") || value == "1" => Ok(true),
        Some(value) if value.eq_ignore_ascii_case("false") || value == "0" => Ok(false),
        Some(value) => Err(format!("{name} must be true or false, got {value:?}")),
        None => Ok(default),
    }
}
