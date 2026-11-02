use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Barrier, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use super::*;
use crate::{
    cancellation::CancellationToken,
    domain::{Location, RoutingReference, TimeOfDay, TimeWindow},
    matrix::TravelTimeMatrix,
};

fn locations() -> Vec<Location> {
    locations_n(3)
}

fn locations_n(count: usize) -> Vec<Location> {
    (0..count)
        .map(|index| {
            Location::new(
                index.to_string(),
                RoutingReference::GooglePlaceId(index.to_string()),
                TimeWindow::new(
                    TimeOfDay::from_minutes(0).unwrap(),
                    TimeOfDay::from_minutes(1439).unwrap(),
                )
                .unwrap(),
                0,
            )
        })
        .collect()
}

fn pair_minutes(from: &Location, to: &Location) -> u32 {
    from.id().parse::<u32>().unwrap() * 10 + to.id().parse::<u32>().unwrap()
}

#[derive(Default)]
struct DirectedPairs {
    calls: Mutex<Vec<(String, String)>>,
}

impl TravelTimeProvider for DirectedPairs {
    fn travel_time(
        &self,
        from: &Location,
        to: &Location,
        _context: &RoutingContext,
    ) -> Result<TravelTime, RoutingError> {
        self.calls
            .lock()
            .unwrap()
            .push((from.id().to_owned(), to.id().to_owned()));
        Ok(TravelTime {
            minutes: pair_minutes(from, to),
        })
    }
}

include!("tests/matrix.rs");
include!("tests/failures.rs");
