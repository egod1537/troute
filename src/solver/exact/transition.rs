use super::super::{SolverInput, TIME_SLOT_MINUTES};

#[derive(Debug, Clone, Copy)]
pub(in crate::solver) struct InitialService {
    pub departure_slot: u16,
    pub wait_minutes: u32,
}

/// Applies the start location's opening window and service duration to a
/// candidate visit-start slot. The returned slot is the first departure slot.
pub(in crate::solver) fn initial_service(
    input: &SolverInput<'_>,
    start_time_slot: u16,
) -> Option<InitialService> {
    let location = input.problem.start_location();
    let arrival = u32::from(start_time_slot);
    let service_start = arrival.max(minutes_to_slot_ceil(u32::from(
        location.time_window().open().minutes(),
    )));
    let departure = service_start.checked_add(minutes_to_slot_ceil(location.stay_minutes()))?;
    let close = u32::from(location.time_window().close().minutes()) / TIME_SLOT_MINUTES;
    if departure > close || departure >= slots_per_day() {
        return None;
    }
    Some(InitialService {
        departure_slot: departure as u16,
        wait_minutes: (service_start - arrival).checked_mul(TIME_SLOT_MINUTES)?,
    })
}

pub(in crate::solver) fn transition_time(
    input: &SolverInput<'_>,
    from: usize,
    to: usize,
    time_slot: u16,
) -> Option<u16> {
    let travel = input.matrix.travel_minutes(from, to)?;
    let location = &input.problem.locations()[to];
    let arrival = u32::from(time_slot).checked_add(minutes_to_slot_ceil(travel))?;
    let service_start = arrival.max(minutes_to_slot_ceil(u32::from(
        location.time_window().open().minutes(),
    )));
    let finish = service_start.checked_add(minutes_to_slot_ceil(location.stay_minutes()))?;
    let close = u32::from(location.time_window().close().minutes()) / TIME_SLOT_MINUTES;
    (finish <= close && finish < slots_per_day()).then_some(finish as u16)
}

pub(in crate::solver) fn minutes_to_slot_ceil(minutes: u32) -> u32 {
    minutes.div_ceil(TIME_SLOT_MINUTES)
}

pub(in crate::solver) fn slots_per_day() -> u32 {
    (24 * 60) / TIME_SLOT_MINUTES
}
