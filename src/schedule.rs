use thiserror::Error;

use crate::{
    domain::{OptimizationProblem, RoutePlan, ScheduledStop},
    matrix::TravelTimeMatrix,
    solver::SolverSolution,
};

/// Expands a solver order into arrival and departure times.
///
/// In v0 the start location is a depot: its opening hours and stay duration are
/// ignored at initial departure and final return.
pub fn calculate_schedule(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    solution: &SolverSolution,
) -> Result<RoutePlan, ScheduleError> {
    validate_solution(problem, matrix, solution)?;

    let mut current_time = problem.start_time();
    let mut total_travel_minutes = 0_u32;
    let mut stops = Vec::with_capacity(solution.visit_order.len());
    stops.push(ScheduledStop {
        location_index: problem.start_index(),
        arrival_time: current_time,
        departure_time: Some(current_time),
    });

    for (position, edge) in solution.visit_order.windows(2).enumerate() {
        let from = edge[0];
        let to = edge[1];
        let travel_minutes = matrix
            .travel_minutes(from, to)
            .expect("solution and matrix indices were validated");

        total_travel_minutes = total_travel_minutes
            .checked_add(travel_minutes)
            .ok_or(ScheduleError::TravelTimeOverflow)?;
        let arrival_time = current_time
            .checked_add(travel_minutes)
            .ok_or(ScheduleError::OutsideSingleDay)?;
        let is_final_return = position + 2 == solution.visit_order.len();

        if is_final_return {
            current_time = arrival_time;
            stops.push(ScheduledStop {
                location_index: to,
                arrival_time,
                departure_time: None,
            });
            continue;
        }

        let location = &problem.locations()[to];
        let service_start = arrival_time.max(location.time_window().open());
        let departure_time = service_start
            .checked_add(location.stay_minutes())
            .ok_or(ScheduleError::OutsideSingleDay)?;

        if departure_time > location.time_window().close() {
            return Err(ScheduleError::TimeWindowViolation {
                location_id: location.id().to_owned(),
            });
        }

        current_time = departure_time;
        stops.push(ScheduledStop {
            location_index: to,
            arrival_time,
            departure_time: Some(departure_time),
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

    if solution.visit_order.len() != location_count + 1
        || solution.visit_order.first() != Some(&problem.start_index())
        || solution.visit_order.last() != Some(&problem.start_index())
    {
        return Err(ScheduleError::InvalidVisitOrder);
    }

    let mut seen = vec![false; location_count];
    seen[problem.start_index()] = true;
    for &index in &solution.visit_order[1..solution.visit_order.len() - 1] {
        if index >= location_count || index == problem.start_index() || seen[index] {
            return Err(ScheduleError::InvalidVisitOrder);
        }
        seen[index] = true;
    }

    if seen.iter().any(|visited| !visited) {
        return Err(ScheduleError::InvalidVisitOrder);
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum ScheduleError {
    #[error("matrix size {matrix} does not match {locations} locations")]
    MatrixSizeMismatch { matrix: usize, locations: usize },
    #[error("visit order must start and end at the depot and visit every other location once")]
    InvalidVisitOrder,
    #[error("route exceeds the single-day time range supported by v0")]
    OutsideSingleDay,
    #[error("travel time total overflowed")]
    TravelTimeOverflow,
    #[error("route violates the time window for location {location_id}")]
    TimeWindowViolation { location_id: String },
}
