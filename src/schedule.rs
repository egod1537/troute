use thiserror::Error;

use crate::{
    domain::{OptimizationProblem, RoutePlan, ScheduledStop},
    matrix::TravelTimeMatrix,
    solver::SolverSolution,
};

/// Expands a solver order into arrival and departure times.
///
/// The first and last locations are fixed endpoints. The start location's
/// opening hours and stay duration are ignored at initial departure; every
/// subsequent location, including the destination, keeps its service rules.
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
        location_index: problem.start_location_index(),
        arrival_time: current_time,
        departure_time: Some(current_time),
    });

    for edge in solution.visit_order.windows(2) {
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

    if solution.visit_order.len() != location_count
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

    if seen.iter().any(|visited| !visited) {
        return Err(ScheduleError::InvalidVisitOrder);
    }

    Ok(())
}

#[derive(Debug, Error)]
pub enum ScheduleError {
    #[error("matrix size {matrix} does not match {locations} locations")]
    MatrixSizeMismatch { matrix: usize, locations: usize },
    #[error("visit order must keep the first and last locations fixed and visit every intermediate location once")]
    InvalidVisitOrder,
    #[error("route exceeds the single-day time range supported by v0")]
    OutsideSingleDay,
    #[error("travel time total overflowed")]
    TravelTimeOverflow,
    #[error("route violates the time window for location {location_id}")]
    TimeWindowViolation { location_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::OptimizeRouteRequest;

    fn problem() -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-schedule-test",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:59","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:59","stay_minutes":10},
                    {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:59","stay_minutes":20},
                    {"id":"D","place_id":"d","open_time":"00:00","close_time":"23:59","stay_minutes":0}
                ],
                "start_time":"09:00"
            }"#,
        )
        .unwrap();
        request.try_into().unwrap()
    }

    fn matrix() -> TravelTimeMatrix {
        TravelTimeMatrix::new(vec![vec![0; 4]; 4]).unwrap()
    }

    #[test]
    fn reordered_intermediates_keep_both_endpoints_fixed() {
        let problem = problem();
        let solution = SolverSolution {
            visit_order: vec![0, 2, 1, 3],
        };
        let plan = calculate_schedule(&problem, &matrix(), &solution).unwrap();

        assert_eq!(
            plan.stops
                .iter()
                .map(|stop| stop.location_index)
                .collect::<Vec<_>>(),
            vec![0, 2, 1, 3]
        );
        assert_eq!(plan.stops[0].departure_time.unwrap().to_string(), "09:00");
        assert_eq!(plan.stops[1].departure_time.unwrap().to_string(), "09:20");
        assert_eq!(plan.stops[2].departure_time.unwrap().to_string(), "09:30");
        assert_eq!(plan.stops[3].departure_time.unwrap().to_string(), "09:30");
    }

    #[test]
    fn moved_start_or_end_is_rejected() {
        let problem = problem();
        for visit_order in [vec![1, 0, 2, 3], vec![0, 1, 3, 2]] {
            let error = calculate_schedule(&problem, &matrix(), &SolverSolution { visit_order })
                .unwrap_err();
            assert!(matches!(error, ScheduleError::InvalidVisitOrder));
        }
    }
}
