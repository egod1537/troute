use std::{fmt, str::FromStr};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

const MINUTES_PER_DAY: u32 = 24 * 60;

/// A local wall-clock time in the single-day v0 model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeOfDay(u16);

impl TimeOfDay {
    pub fn from_minutes(minutes: u16) -> Result<Self, TimeParseError> {
        if u32::from(minutes) >= MINUTES_PER_DAY {
            return Err(TimeParseError::OutOfRange);
        }
        Ok(Self(minutes))
    }

    pub fn minutes(self) -> u16 {
        self.0
    }

    pub fn checked_add(self, minutes: u32) -> Option<Self> {
        let result = u32::from(self.0).checked_add(minutes)?;
        if result >= MINUTES_PER_DAY {
            return None;
        }
        Some(Self(result as u16))
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:02}:{:02}", self.0 / 60, self.0 % 60)
    }
}

impl FromStr for TimeOfDay {
    type Err = TimeParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (hours, minutes) = value.split_once(':').ok_or(TimeParseError::InvalidFormat)?;

        if hours.len() != 2 || minutes.len() != 2 {
            return Err(TimeParseError::InvalidFormat);
        }

        let hours: u16 = hours.parse().map_err(|_| TimeParseError::InvalidFormat)?;
        let minutes: u16 = minutes.parse().map_err(|_| TimeParseError::InvalidFormat)?;

        if hours >= 24 || minutes >= 60 {
            return Err(TimeParseError::OutOfRange);
        }

        Ok(Self(hours * 60 + minutes))
    }
}

impl Serialize for TimeOfDay {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TimeParseError {
    #[error("time must use the HH:MM format")]
    InvalidFormat,
    #[error("time must be between 00:00 and 23:59")]
    OutOfRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingReference {
    GooglePlaceId(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeWindow {
    open: TimeOfDay,
    close: TimeOfDay,
}

impl TimeWindow {
    pub fn new(open: TimeOfDay, close: TimeOfDay) -> Result<Self, DomainError> {
        if open > close {
            return Err(DomainError::InvalidTimeWindow);
        }
        Ok(Self { open, close })
    }

    pub fn open(self) -> TimeOfDay {
        self.open
    }

    pub fn close(self) -> TimeOfDay {
        self.close
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    id: String,
    routing_reference: RoutingReference,
    time_window: TimeWindow,
    stay_minutes: u32,
}

impl Location {
    pub fn new(
        id: String,
        routing_reference: RoutingReference,
        time_window: TimeWindow,
        stay_minutes: u32,
    ) -> Self {
        Self {
            id,
            routing_reference,
            time_window,
            stay_minutes,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn routing_reference(&self) -> &RoutingReference {
        &self.routing_reference
    }

    pub fn time_window(&self) -> TimeWindow {
        self.time_window
    }

    pub fn stay_minutes(&self) -> u32 {
        self.stay_minutes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizationProblem {
    locations: Vec<Location>,
    start_index: usize,
    start_time: TimeOfDay,
}

impl OptimizationProblem {
    pub(crate) fn new(locations: Vec<Location>, start_index: usize, start_time: TimeOfDay) -> Self {
        Self {
            locations,
            start_index,
            start_time,
        }
    }

    pub fn locations(&self) -> &[Location] {
        &self.locations
    }

    pub fn start_index(&self) -> usize {
        self.start_index
    }

    pub fn start_time(&self) -> TimeOfDay {
        self.start_time
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledStop {
    pub location_index: usize,
    pub arrival_time: TimeOfDay,
    pub departure_time: Option<TimeOfDay>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePlan {
    pub stops: Vec<ScheduledStop>,
    pub total_travel_minutes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error("opening time must not be later than closing time in v0")]
    InvalidTimeWindow,
}

#[cfg(test)]
mod tests {
    use super::TimeOfDay;

    #[test]
    fn time_of_day_serializes_as_hhmm() {
        let time = TimeOfDay::from_minutes(13 * 60 + 45).unwrap();
        assert_eq!(serde_json::to_string(&time).unwrap(), r#""13:45""#);
    }

    #[test]
    fn time_of_day_deserializes_valid_hhmm() {
        for (json, expected_minutes) in [
            (r#""00:00""#, 0),
            (r#""09:30""#, 9 * 60 + 30),
            (r#""23:59""#, 23 * 60 + 59),
        ] {
            let time: TimeOfDay = serde_json::from_str(json).unwrap();
            assert_eq!(time.minutes(), expected_minutes);
        }
    }

    #[test]
    fn time_of_day_rejects_invalid_wire_values() {
        for json in [r#""9:30""#, r#""24:00""#, r#""09:60""#, "570"] {
            assert!(serde_json::from_str::<TimeOfDay>(json).is_err(), "{json}");
        }
    }
}
