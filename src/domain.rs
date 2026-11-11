use std::{fmt, str::FromStr};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

const MINUTES_PER_DAY: u32 = 24 * 60;

/// Controls how the itinerary departure time participates in optimization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StartPolicy {
    Fixed,
    Earliest,
    #[default]
    Latest,
}

/// Controls whether intermediate locations may be omitted from a successful
/// itinerary. The legacy default keeps every location required.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VisitPolicy {
    #[default]
    AllRequired,
    BestEffortOnFailure,
}

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
    required: bool,
    priority: u8,
    order_locked: bool,
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
            required: true,
            priority: 1,
            order_locked: false,
        }
    }

    pub(crate) fn with_visit_metadata(mut self, required: bool, priority: u8) -> Self {
        self.required = required;
        self.priority = priority;
        self
    }

    pub(crate) fn with_order_lock(mut self, locked: bool) -> Self {
        self.order_locked = locked;
        self
    }

    pub fn order_locked(&self) -> bool {
        self.order_locked
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

    pub fn required(&self) -> bool {
        self.required
    }

    pub fn priority(&self) -> u8 {
        self.priority
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizationProblem {
    locations: Vec<Location>,
    start_time: TimeOfDay,
    start_policy: StartPolicy,
    visit_policy: VisitPolicy,
}

impl OptimizationProblem {
    #[cfg(test)]
    pub(crate) fn new(locations: Vec<Location>, start_time: TimeOfDay) -> Self {
        Self::with_start_policy(locations, start_time, StartPolicy::Latest)
    }

    pub(crate) fn with_start_policy(
        locations: Vec<Location>,
        start_time: TimeOfDay,
        start_policy: StartPolicy,
    ) -> Self {
        Self::with_policies(
            locations,
            start_time,
            start_policy,
            VisitPolicy::AllRequired,
        )
    }

    pub(crate) fn with_policies(
        locations: Vec<Location>,
        start_time: TimeOfDay,
        start_policy: StartPolicy,
        visit_policy: VisitPolicy,
    ) -> Self {
        debug_assert!(!locations.is_empty());
        Self {
            locations,
            start_time,
            start_policy,
            visit_policy,
        }
    }

    /// A locked location keeps its zero-based position in the submitted list.
    pub fn allows_visit_at(&self, location: usize, position: usize) -> bool {
        self.locations.get(location).is_some_and(|value| {
            (!value.order_locked() || location == position)
                && self
                    .locations
                    .get(position)
                    .is_some_and(|occupant| !occupant.order_locked() || location == position)
        })
    }

    pub fn preserves_order_locks(&self, order: &[usize]) -> bool {
        self.locations
            .iter()
            .enumerate()
            .all(|(position, location)| {
                !location.order_locked() || order.get(position) == Some(&position)
            })
    }

    pub fn has_order_locks(&self) -> bool {
        self.locations.iter().any(Location::order_locked)
    }

    /// Project a complete heuristic seed onto the fixed visit positions.
    /// Partial orders are left to the constrained search and shared validator.
    pub(crate) fn restore_order_locks(&self, order: &mut [usize]) {
        if order.len() != self.locations.len() {
            return;
        }
        for (position, location) in self.locations.iter().enumerate() {
            if location.order_locked() {
                if let Some(current) = order.iter().position(|&index| index == position) {
                    order.swap(position, current);
                }
            }
        }
    }

    pub fn locations(&self) -> &[Location] {
        &self.locations
    }

    pub fn start_location(&self) -> &Location {
        self.locations
            .first()
            .expect("optimization problems contain a start location")
    }

    pub fn end_location(&self) -> &Location {
        self.locations
            .last()
            .expect("optimization problems contain an end location")
    }

    pub fn intermediate_locations(&self) -> &[Location] {
        if self.locations.len() <= 2 {
            &[]
        } else {
            &self.locations[1..self.locations.len() - 1]
        }
    }

    pub fn start_location_index(&self) -> usize {
        0
    }

    pub fn end_location_index(&self) -> usize {
        self.locations.len() - 1
    }

    pub fn start_time(&self) -> TimeOfDay {
        self.start_time
    }

    pub fn start_policy(&self) -> StartPolicy {
        self.start_policy
    }

    pub fn visit_policy(&self) -> VisitPolicy {
        self.visit_policy
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledStop {
    pub location_index: usize,
    pub arrival_time: TimeOfDay,
    pub service_start_time: TimeOfDay,
    pub departure_time: Option<TimeOfDay>,
    pub wait_minutes: u32,
    pub stay_minutes: u32,
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
mod tests;
