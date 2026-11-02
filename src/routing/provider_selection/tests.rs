use super::*;
use crate::routing::{RoutingError, TravelMode};

fn selector(policy: Option<&str>) -> RouteProviderSelector {
    RouteProviderSelector::from_config_sources(
        "auto",
        policy,
        None,
        false,
        RouteProviderRegistry::default(),
    )
    .unwrap()
}

fn resolve(
    selector: &RouteProviderSelector,
    country: Option<&str>,
    mode: TravelMode,
) -> RouteProviderSelection {
    selector
        .resolve(&RouteProviderResolutionContext {
            country_code: country.map(str::to_owned),
            mode,
            request_override: None,
        })
        .unwrap()
}

#[test]
fn built_in_country_mode_policy_matches_japan_and_korea() {
    let selector = selector(None);
    assert_eq!(
        resolve(&selector, Some("jp"), TravelMode::Transit).provider,
        RouteProviderName::Ekispert
    );
    assert_eq!(
        resolve(&selector, Some("JP"), TravelMode::Driving).provider,
        RouteProviderName::Google
    );
    assert_eq!(
        resolve(&selector, Some("KR"), TravelMode::Driving).provider,
        RouteProviderName::KakaoMobility
    );
    assert_eq!(
        resolve(&selector, Some("KR"), TravelMode::Transit).provider,
        RouteProviderName::KakaoMaps
    );
}

#[test]
fn resolution_priority_covers_every_policy_level_and_missing_policy() {
    let configured = selector(Some(
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

    let missing = selector(Some(r#"{"countries":{}}"#));
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
    let disabled = selector(None);
    assert!(matches!(
        disabled.resolve(&context),
        Err(RoutingError::ProviderOverrideDisabled)
    ));
    let enabled = RouteProviderSelector::from_config_sources(
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
    let forced = RouteProviderSelector::from_config_sources(
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
    assert!(RouteProviderSelector::from_config_sources(
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
        assert!(RouteProviderSelector::from_config_sources(
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
    let mismatch = selector(Some(r#"{"defaultProvider":"otp"}"#));
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
    let unavailable = RouteProviderSelector::from_config_sources(
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
    let legacy = RouteProviderSelector::from_config_sources(
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

    let explicit = RouteProviderSelector::from_config_sources(
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
