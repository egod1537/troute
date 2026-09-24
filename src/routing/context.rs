use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Options which affect a routing lookup but are independent of optimization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingContext {
    pub departure_time: Option<DateTime<Utc>>,
    pub travel_mode: TravelMode,
    pub timezone: Option<String>,
    pub options: BTreeMap<String, String>,
}

impl Default for RoutingContext {
    fn default() -> Self {
        Self {
            departure_time: None,
            travel_mode: TravelMode::Transit,
            timezone: None,
            options: BTreeMap::new(),
        }
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TravelMode {
    #[default]
    Transit,
    Driving,
    Walking,
    Bicycling,
}

impl TravelMode {
    pub fn as_provider_value(self) -> &'static str {
        match self {
            Self::Transit => "TRANSIT",
            Self::Driving => "DRIVING",
            Self::Walking => "WALKING",
            Self::Bicycling => "BICYCLING",
        }
    }
}
