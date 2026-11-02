use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::routing::TravelMode;

use super::types::{normalize_country_code, RouteProviderName};

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

    pub(super) fn normalize(mut self) -> Result<Self, String> {
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
