use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::routing::TravelMode;

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
