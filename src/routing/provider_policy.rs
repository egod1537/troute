use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    str::FromStr,
};

use serde::{Deserialize, Serialize};

use super::{RoutingContext, RoutingError, RoutingProvider, TravelMode};
use crate::{domain::Location, matrix::TravelTimeMatrix};

const ISO_COUNTRY_CODES: &str = "AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ BR BS BT BV BW BY BZ CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX CY CZ DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI FJ FK FM FO FR GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY HK HM HN HR HT HU ID IE IL IM IN IO IQ IR IS IT JE JM JO JP KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MF MG MH MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ NA NC NE NF NG NI NL NO NP NR NU NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY QA RE RO RS RU RW SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ TC TD TF TG TH TJ TK TL TM TN TO TR TT TV TW TZ UA UG UM US UY UZ VA VC VE VG VI VN VU WF WS YE YT ZA ZM ZW";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteProviderName {
    Google,
    KakaoMobility,
    KakaoMaps,
    Ekispert,
    Navitime,
    Otp,
}

impl RouteProviderName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::KakaoMobility => "kakao-mobility",
            Self::KakaoMaps => "kakao-maps",
            Self::Ekispert => "ekispert",
            Self::Navitime => "navitime",
            Self::Otp => "otp",
        }
    }
}

impl std::fmt::Display for RouteProviderName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for RouteProviderName {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "google" => Ok(Self::Google),
            "kakao-mobility" => Ok(Self::KakaoMobility),
            "kakao-maps" => Ok(Self::KakaoMaps),
            "ekispert" => Ok(Self::Ekispert),
            "navitime" => Ok(Self::Navitime),
            "otp" => Ok(Self::Otp),
            _ => Err(format!("unknown route provider {value:?}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CountryRouteProviderPolicy {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub modes: BTreeMap<TravelMode, RouteProviderName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<RouteProviderName>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteProviderPolicy {
    #[serde(default)]
    pub countries: BTreeMap<String, CountryRouteProviderPolicy>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub mode_defaults: BTreeMap<TravelMode, RouteProviderName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<RouteProviderName>,
}

impl RouteProviderPolicy {
    pub fn built_in() -> Self {
        let modes = |values: &[(TravelMode, RouteProviderName)]| values.iter().copied().collect();
        Self {
            countries: BTreeMap::from([
                (
                    "JP".to_owned(),
                    CountryRouteProviderPolicy {
                        modes: modes(&[
                            (TravelMode::Driving, RouteProviderName::Google),
                            (TravelMode::Walking, RouteProviderName::Google),
                            (TravelMode::Bicycling, RouteProviderName::Google),
                            (TravelMode::Transit, RouteProviderName::Ekispert),
                        ]),
                        default_provider: None,
                    },
                ),
                (
                    "KR".to_owned(),
                    CountryRouteProviderPolicy {
                        modes: modes(&[
                            (TravelMode::Driving, RouteProviderName::KakaoMobility),
                            (TravelMode::Walking, RouteProviderName::KakaoMaps),
                            (TravelMode::Bicycling, RouteProviderName::KakaoMaps),
                            (TravelMode::Transit, RouteProviderName::KakaoMaps),
                        ]),
                        default_provider: None,
                    },
                ),
            ]),
            mode_defaults: BTreeMap::new(),
            default_provider: Some(RouteProviderName::Google),
        }
    }

    fn normalize(mut self) -> Result<Self, String> {
        let mut countries = BTreeMap::new();
        for (raw_country, policy) in self.countries {
            let country = normalize_country_code(&raw_country)?;
            if countries.insert(country.clone(), policy).is_some() {
                return Err(format!(
                    "route provider policy contains duplicate normalized country {country}"
                ));
            }
        }
        self.countries = countries;
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteProviderMode {
    Auto,
    Force(RouteProviderName),
}

impl RouteProviderMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Force(provider) => provider.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouteProviderSelectionSource {
    RequestOverride,
    GlobalForce,
    CountryMode,
    CountryDefault,
    ModeDefault,
    GlobalDefault,
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteProviderSelection {
    pub provider: RouteProviderName,
    pub reason: String,
    pub source: RouteProviderSelectionSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteProviderResolutionContext {
    pub country_code: Option<String>,
    pub mode: TravelMode,
    pub request_override: Option<RouteProviderName>,
}

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
pub struct RouteProviderPolicyDiagnostics {
    pub route_provider_mode: String,
    pub override_enabled: bool,
    pub policy: RouteProviderPolicy,
    pub providers: Vec<ProviderAvailabilityDiagnostic>,
    pub routes: Vec<PolicyRouteDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProviderRegistration {
    available: bool,
    unavailable_reason: Option<String>,
    capabilities: Vec<TravelMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteProviderRegistry {
    providers: BTreeMap<RouteProviderName, ProviderRegistration>,
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

    fn validate_selection(
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteProviderPolicyResolver {
    mode: RouteProviderMode,
    policy: RouteProviderPolicy,
    override_enabled: bool,
    registry: RouteProviderRegistry,
    legacy_routes: BTreeSet<(String, TravelMode)>,
}

impl RouteProviderPolicyResolver {
    pub fn from_env() -> Result<Self, String> {
        let registry = RouteProviderRegistry::default();
        let route_provider = env::var("ROUTE_PROVIDER").unwrap_or_else(|_| "auto".to_owned());
        let policy_json = env::var("ROUTE_PROVIDER_POLICY_JSON").ok();
        let legacy = env::var("JAPAN_TRANSIT_PROVIDER").ok();
        let override_enabled = parse_bool_env("ROUTE_PROVIDER_OVERRIDE_ENABLED", false)?;
        Self::from_config_sources(
            &route_provider,
            policy_json.as_deref(),
            legacy.as_deref(),
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

    pub fn diagnostics(&self) -> RouteProviderPolicyDiagnostics {
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
        RouteProviderPolicyDiagnostics {
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

pub fn normalize_country_code(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_uppercase();
    if normalized.len() != 2
        || !normalized.bytes().all(|byte| byte.is_ascii_uppercase())
        || !ISO_COUNTRY_CODES
            .split_ascii_whitespace()
            .any(|code| code == normalized)
    {
        return Err(format!(
            "countryCode must be a valid ISO 3166-1 alpha-2 code: {value:?}"
        ));
    }
    Ok(normalized)
}

fn parse_bool_env(name: &str, default: bool) -> Result<bool, String> {
    match env::var(name) {
        Ok(value) if value.eq_ignore_ascii_case("true") || value == "1" => Ok(true),
        Ok(value) if value.eq_ignore_ascii_case("false") || value == "0" => Ok(false),
        Ok(value) => Err(format!("{name} must be true or false, got {value:?}")),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Debug, Clone)]
pub struct PolicyRoutingProvider<P> {
    inner: P,
    resolver: RouteProviderPolicyResolver,
}

impl<P> PolicyRoutingProvider<P> {
    pub fn new(inner: P, resolver: RouteProviderPolicyResolver) -> Self {
        Self { inner, resolver }
    }

    pub fn resolver(&self) -> &RouteProviderPolicyResolver {
        &self.resolver
    }
}

impl<P: RoutingProvider> RoutingProvider for PolicyRoutingProvider<P> {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let selection = self.provider_selection(context)?.ok_or_else(|| {
            RoutingError::ProviderResolution("provider policy returned no selection".to_owned())
        })?;
        let mut resolved = context.clone();
        resolved
            .options
            .insert("routeProvider".to_owned(), selection.provider.to_string());
        self.inner.travel_time_matrix(locations, &resolved)
    }

    fn travel_time_matrix_until(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        deadline: std::time::Instant,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let selection = self.provider_selection(context)?.ok_or_else(|| {
            RoutingError::ProviderResolution("provider policy returned no selection".to_owned())
        })?;
        let mut resolved = context.clone();
        resolved
            .options
            .insert("routeProvider".to_owned(), selection.provider.to_string());
        self.inner
            .travel_time_matrix_until(locations, &resolved, deadline)
    }

    fn provider_selection(
        &self,
        context: &RoutingContext,
    ) -> Result<Option<RouteProviderSelection>, RoutingError> {
        self.resolver
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

    fn provider_policy_diagnostics(&self) -> Option<RouteProviderPolicyDiagnostics> {
        Some(self.resolver.diagnostics())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(policy: Option<&str>) -> RouteProviderPolicyResolver {
        RouteProviderPolicyResolver::from_config_sources(
            "auto",
            policy,
            None,
            false,
            RouteProviderRegistry::default(),
        )
        .unwrap()
    }

    fn resolve(
        resolver: &RouteProviderPolicyResolver,
        country: Option<&str>,
        mode: TravelMode,
    ) -> RouteProviderSelection {
        resolver
            .resolve(&RouteProviderResolutionContext {
                country_code: country.map(str::to_owned),
                mode,
                request_override: None,
            })
            .unwrap()
    }

    #[test]
    fn built_in_country_mode_policy_matches_japan_and_korea() {
        let resolver = resolver(None);
        assert_eq!(
            resolve(&resolver, Some("jp"), TravelMode::Transit).provider,
            RouteProviderName::Ekispert
        );
        assert_eq!(
            resolve(&resolver, Some("JP"), TravelMode::Driving).provider,
            RouteProviderName::Google
        );
        assert_eq!(
            resolve(&resolver, Some("KR"), TravelMode::Driving).provider,
            RouteProviderName::KakaoMobility
        );
        assert_eq!(
            resolve(&resolver, Some("KR"), TravelMode::Transit).provider,
            RouteProviderName::KakaoMaps
        );
    }

    #[test]
    fn resolution_priority_covers_every_policy_level_and_missing_policy() {
        let configured = resolver(Some(
            r#"{"countries":{"JP":{"modes":{"TRANSIT":"ekispert"},"defaultProvider":"google"}},"modeDefaults":{"BICYCLING":"navitime"},"defaultProvider":"google"}"#,
        ));
        assert_eq!(
            resolve(&configured, Some("JP"), TravelMode::Transit).source,
            RouteProviderSelectionSource::CountryMode
        );
        assert_eq!(
            resolve(&configured, Some("JP"), TravelMode::Walking).source,
            RouteProviderSelectionSource::CountryDefault
        );
        assert_eq!(
            resolve(&configured, Some("US"), TravelMode::Bicycling).source,
            RouteProviderSelectionSource::ModeDefault
        );
        assert_eq!(
            resolve(&configured, Some("US"), TravelMode::Driving).source,
            RouteProviderSelectionSource::GlobalDefault
        );

        let missing = resolver(Some(r#"{"countries":{}}"#));
        assert!(matches!(
            missing.resolve(&RouteProviderResolutionContext {
                country_code: None,
                mode: TravelMode::Transit,
                request_override: None,
            }),
            Err(RoutingError::ProviderResolution(_))
        ));
    }

    #[test]
    fn request_override_and_global_force_obey_configuration() {
        let context = RouteProviderResolutionContext {
            country_code: Some("JP".to_owned()),
            mode: TravelMode::Transit,
            request_override: Some(RouteProviderName::Google),
        };
        let disabled = resolver(None);
        assert!(matches!(
            disabled.resolve(&context),
            Err(RoutingError::ProviderOverrideDisabled)
        ));
        let enabled = RouteProviderPolicyResolver::from_config_sources(
            "auto",
            None,
            None,
            true,
            RouteProviderRegistry::default(),
        )
        .unwrap();
        assert_eq!(
            enabled.resolve(&context).unwrap().source,
            RouteProviderSelectionSource::RequestOverride
        );
        let forced = RouteProviderPolicyResolver::from_config_sources(
            "google",
            None,
            None,
            false,
            RouteProviderRegistry::default(),
        )
        .unwrap();
        assert_eq!(
            forced.resolve(&context).unwrap().source,
            RouteProviderSelectionSource::GlobalForce
        );
    }

    #[test]
    fn invalid_json_country_mode_and_provider_are_rejected() {
        assert!(RouteProviderPolicyResolver::from_config_sources(
            "auto",
            Some("{"),
            None,
            false,
            RouteProviderRegistry::default()
        )
        .is_err());
        for json in [
            r#"{"countries":{"XX":{"modes":{"TRANSIT":"google"}}}}"#,
            r#"{"countries":{},"modeDefaults":{"FLYING":"google"}}"#,
            r#"{"countries":{},"defaultProvider":"unknown"}"#,
        ] {
            assert!(RouteProviderPolicyResolver::from_config_sources(
                "auto",
                Some(json),
                None,
                false,
                RouteProviderRegistry::default()
            )
            .is_err());
        }
    }

    #[test]
    fn capability_and_availability_are_not_silently_fallbacked() {
        let mismatch = resolver(Some(r#"{"defaultProvider":"otp"}"#));
        assert!(matches!(
            mismatch.resolve(&RouteProviderResolutionContext {
                country_code: None,
                mode: TravelMode::Driving,
                request_override: None,
            }),
            Err(RoutingError::UnsupportedProviderCapability { .. })
        ));
        let mut registry = RouteProviderRegistry::default();
        registry.set_availability(
            RouteProviderName::Google,
            false,
            Some("GOOGLE_API_KEY missing".to_owned()),
        );
        let unavailable = RouteProviderPolicyResolver::from_config_sources(
            "auto",
            Some(r#"{"defaultProvider":"google"}"#),
            None,
            false,
            registry,
        )
        .unwrap();
        assert!(matches!(
            unavailable.resolve(&RouteProviderResolutionContext {
                country_code: None,
                mode: TravelMode::Driving,
                request_override: None,
            }),
            Err(RoutingError::ProviderNotConfigured { .. })
        ));
    }

    #[test]
    fn legacy_japan_transit_only_fills_a_missing_explicit_rule() {
        let legacy = RouteProviderPolicyResolver::from_config_sources(
            "auto",
            Some(r#"{"defaultProvider":"google"}"#),
            Some("navitime"),
            false,
            RouteProviderRegistry::default(),
        )
        .unwrap();
        let selected = resolve(&legacy, Some("JP"), TravelMode::Transit);
        assert_eq!(selected.provider, RouteProviderName::Navitime);
        assert_eq!(selected.source, RouteProviderSelectionSource::Legacy);

        let explicit = RouteProviderPolicyResolver::from_config_sources(
            "auto",
            Some(r#"{"countries":{"JP":{"modes":{"TRANSIT":"ekispert"}}}}"#),
            Some("navitime"),
            false,
            RouteProviderRegistry::default(),
        )
        .unwrap();
        assert_eq!(
            resolve(&explicit, Some("JP"), TravelMode::Transit).provider,
            RouteProviderName::Ekispert
        );
    }
}
