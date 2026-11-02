use thiserror::Error;

use crate::{
    domain::{OptimizationProblem, RoutePlan, ScheduledStop},
    matrix::TravelTimeMatrix,
    solver::SolverSolution,
};

/// Expands a solver order into arrival and departure times.
///
/// The first and last locations are fixed endpoints. Every location, including
/// the start and destination, observes its opening hours and stay duration.
pub fn calculate_schedule(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    solution: &SolverSolution,
) -> Result<RoutePlan, ScheduleError> {
    calculate_schedule_from(problem, matrix, solution, problem.start_time())
}

/// Expands an order using a solver-selected visit-start time.
pub fn calculate_schedule_from(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    solution: &SolverSolution,
    start_time: crate::domain::TimeOfDay,
) -> Result<RoutePlan, ScheduleError> {
    validate_solution(problem, matrix, solution)?;

    let start_location = problem.start_location();
    let start_service = start_time.max(start_location.time_window().open());
    let start_wait = u32::from(start_service.minutes() - start_time.minutes());
    let start_departure = start_service
        .checked_add(start_location.stay_minutes())
        .ok_or(ScheduleError::OutsideSingleDay)?;
    if start_departure > start_location.time_window().close() {
        return Err(ScheduleError::TimeWindowViolation {
            location_id: start_location.id().to_owned(),
            arrival_time: start_time,
            service_start_time: start_service,
            required_departure: start_departure,
            close_time: start_location.time_window().close(),
            selected_start_time: start_time,
            total_wait_minutes_before_violation: start_wait,
            is_start: true,
            is_destination: problem.start_location_index() == problem.end_location_index(),
            current_stay_minutes: start_location.stay_minutes(),
            suggested_max_stay_minutes: u32::from(
                start_location
                    .time_window()
                    .close()
                    .minutes()
                    .saturating_sub(start_service.minutes()),
            ),
        });
    }

    let mut current_time = start_departure;
    let mut total_travel_minutes = 0_u32;
    let mut total_wait_minutes = start_wait;
    let mut stops = Vec::with_capacity(solution.visit_order.len());
    stops.push(ScheduledStop {
        location_index: problem.start_location_index(),
        arrival_time: start_time,
        service_start_time: start_service,
        departure_time: Some(start_departure),
        wait_minutes: start_wait,
        stay_minutes: start_location.stay_minutes(),
    });

    for edge in solution.visit_order.windows(2) {
        let from = edge[0];
        let to = edge[1];
        let travel_minutes =
            matrix
                .travel_minutes(from, to)
                .ok_or_else(|| ScheduleError::UnreachableArc {
                    from_location_id: problem.locations()[from].id().to_owned(),
                    to_location_id: problem.locations()[to].id().to_owned(),
                })?;

        total_travel_minutes = total_travel_minutes
            .checked_add(travel_minutes)
            .ok_or(ScheduleError::TravelTimeOverflow)?;
        let arrival_time = current_time
            .checked_add(travel_minutes)
            .ok_or(ScheduleError::OutsideSingleDay)?;
        let location = &problem.locations()[to];
        let service_start = arrival_time.max(location.time_window().open());
        let wait_minutes = u32::from(service_start.minutes() - arrival_time.minutes());
        let departure_time = service_start
            .checked_add(location.stay_minutes())
            .ok_or(ScheduleError::OutsideSingleDay)?;

        if departure_time > location.time_window().close() {
            return Err(ScheduleError::TimeWindowViolation {
                location_id: location.id().to_owned(),
                arrival_time,
                service_start_time: service_start,
                required_departure: departure_time,
                close_time: location.time_window().close(),
                selected_start_time: start_time,
                total_wait_minutes_before_violation: total_wait_minutes
                    .saturating_add(wait_minutes),
                is_start: false,
                is_destination: to == problem.end_location_index(),
                current_stay_minutes: location.stay_minutes(),
                suggested_max_stay_minutes: u32::from(
                    location
                        .time_window()
                        .close()
                        .minutes()
                        .saturating_sub(service_start.minutes()),
                ),
            });
        }

        current_time = departure_time;
        total_wait_minutes = total_wait_minutes.saturating_add(wait_minutes);
        stops.push(ScheduledStop {
            location_index: to,
            arrival_time,
            service_start_time: service_start,
            departure_time: Some(departure_time),
            wait_minutes,
            stay_minutes: location.stay_minutes(),
        });
    }

    Ok(RoutePlan {
        stops,
        total_travel_minutes,
    })
}

fn validate_solution(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    solution: &SolverSolution,
) -> Result<(), ScheduleError> {
    let location_count = problem.locations().len();
    if matrix.size() != location_count {
        return Err(ScheduleError::MatrixSizeMismatch {
            matrix: matrix.size(),
            locations: location_count,
        });
    }

    if location_count == 1 {
        return (solution.visit_order == [0])
            .then_some(())
            .ok_or(ScheduleError::InvalidVisitOrder);
    }

    let all_required = problem.visit_policy() == crate::domain::VisitPolicy::AllRequired;
    if (all_required && solution.visit_order.len() != location_count)
        || solution.visit_order.len() < 2
        || solution.visit_order.first() != Some(&problem.start_location_index())
        || solution.visit_order.last() != Some(&problem.end_location_index())
    {
        return Err(ScheduleError::InvalidVisitOrder);
    }

    let mut seen = vec![false; location_count];
    seen[problem.start_location_index()] = true;
    seen[problem.end_location_index()] = true;
    for &index in &solution.visit_order[1..solution.visit_order.len() - 1] {
        if index >= location_count || seen[index] {
            return Err(ScheduleError::InvalidVisitOrder);
        }
        seen[index] = true;
    }

    if problem
        .locations()
        .iter()
        .enumerate()
        .any(|(index, location)| location.required() && !seen[index])
        || (all_required && seen.iter().any(|visited| !visited))
    {
        return Err(ScheduleError::InvalidVisitOrder);
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum ScheduleError {
    #[error("matrix size {matrix} does not match {locations} locations")]
    MatrixSizeMismatch { matrix: usize, locations: usize },
    #[error(
        "visit order must keep endpoints fixed, avoid duplicates, and include every required location"
    )]
    InvalidVisitOrder,
    #[error("route exceeds the single-day time range supported by v0")]
    OutsideSingleDay,
    #[error("travel time total overflowed")]
    TravelTimeOverflow,
    #[error("route uses unreachable arc {from_location_id} -> {to_location_id}")]
    UnreachableArc {
        from_location_id: String,
        to_location_id: String,
    },
    #[error("route violates the time window for location {location_id}")]
    TimeWindowViolation {
        location_id: String,
        arrival_time: crate::domain::TimeOfDay,
        service_start_time: crate::domain::TimeOfDay,
        required_departure: crate::domain::TimeOfDay,
        close_time: crate::domain::TimeOfDay,
        selected_start_time: crate::domain::TimeOfDay,
        total_wait_minutes_before_violation: u32,
        is_start: bool,
        is_destination: bool,
        current_stay_minutes: u32,
        suggested_max_stay_minutes: u32,
    },
}

#[cfg(test)]
mod tests;
