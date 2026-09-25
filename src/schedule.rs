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

    if solution.visit_order.len() != location_count
        || solution.visit_order.first() != Some(&problem.start_location_index())
        || solution.visit_order.last() != Some(&problem.end_location_index())
    {
        return Err(ScheduleError::InvalidVisitOrder);
    }

    if location_count == 1 {
        return (solution.visit_order == [0])
            .then_some(())
            .ok_or(ScheduleError::InvalidVisitOrder);
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
mod tests {
    use super::*;
    use crate::api::OptimizeRouteRequest;

    fn problem() -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-schedule-test",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":10},
                    {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":20},
                    {"id":"D","place_id":"d","open_time":"00:00","close_time":"23:50","stay_minutes":0}
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

    #[test]
    fn schedule_exposes_service_wait_and_stay_times() {
        let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-schedule-fields",
                "start_policy":"FIXED",
                "start_time":"09:00",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"10:00","close_time":"10:30","stay_minutes":20}
                ]
            }"#,
        )
        .unwrap();
        let problem: OptimizationProblem = request.try_into().unwrap();
        let matrix = TravelTimeMatrix::new(vec![vec![0, 15], vec![15, 0]]).unwrap();
        let plan = calculate_schedule(
            &problem,
            &matrix,
            &SolverSolution {
                visit_order: vec![0, 1],
            },
        )
        .unwrap();

        assert_eq!(plan.stops[1].arrival_time.to_string(), "09:15");
        assert_eq!(plan.stops[1].service_start_time.to_string(), "10:00");
        assert_eq!(plan.stops[1].departure_time.unwrap().to_string(), "10:20");
        assert_eq!(plan.stops[1].wait_minutes, 45);
        assert_eq!(plan.stops[1].stay_minutes, 20);
    }

    #[test]
    fn start_stay_delays_the_first_travel_leg() {
        let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-start-stay",
                "start_policy":"FIXED",
                "start_time":"09:00",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"08:00","close_time":"18:00","stay_minutes":60},
                    {"id":"B","place_id":"b","open_time":"08:00","close_time":"18:00","stay_minutes":0}
                ]
            }"#,
        )
        .unwrap();
        let problem: OptimizationProblem = request.try_into().unwrap();
        let matrix = TravelTimeMatrix::new(vec![vec![0, 20], vec![20, 0]]).unwrap();
        let plan = calculate_schedule(
            &problem,
            &matrix,
            &SolverSolution {
                visit_order: vec![0, 1],
            },
        )
        .unwrap();

        assert_eq!(plan.stops[0].arrival_time.to_string(), "09:00");
        assert_eq!(plan.stops[0].service_start_time.to_string(), "09:00");
        assert_eq!(plan.stops[0].departure_time.unwrap().to_string(), "10:00");
        assert_eq!(plan.stops[0].stay_minutes, 60);
        assert_eq!(plan.stops[1].arrival_time.to_string(), "10:20");
        assert_eq!(plan.total_travel_minutes, 20);
    }

    #[test]
    fn start_opening_time_adds_wait_before_stay() {
        let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-start-wait",
                "start_policy":"FIXED",
                "start_time":"08:30",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"09:00","close_time":"18:00","stay_minutes":30},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0}
                ]
            }"#,
        )
        .unwrap();
        let problem: OptimizationProblem = request.try_into().unwrap();
        let matrix = TravelTimeMatrix::new(vec![vec![0; 2]; 2]).unwrap();
        let plan = calculate_schedule(
            &problem,
            &matrix,
            &SolverSolution {
                visit_order: vec![0, 1],
            },
        )
        .unwrap();

        assert_eq!(plan.stops[0].arrival_time.to_string(), "08:30");
        assert_eq!(plan.stops[0].service_start_time.to_string(), "09:00");
        assert_eq!(plan.stops[0].wait_minutes, 30);
        assert_eq!(plan.stops[0].departure_time.unwrap().to_string(), "09:30");
    }

    #[test]
    fn start_service_must_finish_before_close() {
        let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-start-close",
                "start_policy":"FIXED",
                "start_time":"09:00",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"09:00","close_time":"09:30","stay_minutes":60},
                    {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":0}
                ]
            }"#,
        )
        .unwrap();
        let problem: OptimizationProblem = request.try_into().unwrap();
        let matrix = TravelTimeMatrix::new(vec![vec![0; 2]; 2]).unwrap();
        let error = calculate_schedule(
            &problem,
            &matrix,
            &SolverSolution {
                visit_order: vec![0, 1],
            },
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ScheduleError::TimeWindowViolation { ref location_id, .. } if location_id == "A"
        ));
    }

    #[test]
    fn service_finishing_after_close_is_infeasible() {
        let request: OptimizeRouteRequest = serde_json::from_str(
            r#"{
                "job_id":"route-schedule-close",
                "start_policy":"FIXED",
                "start_time":"09:00",
                "locations":[
                    {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                    {"id":"B","place_id":"b","open_time":"09:00","close_time":"09:20","stay_minutes":10}
                ]
            }"#,
        )
        .unwrap();
        let problem: OptimizationProblem = request.try_into().unwrap();
        let matrix = TravelTimeMatrix::new(vec![vec![0, 15], vec![15, 0]]).unwrap();
        let error = calculate_schedule(
            &problem,
            &matrix,
            &SolverSolution {
                visit_order: vec![0, 1],
            },
        )
        .unwrap_err();
        assert!(matches!(error, ScheduleError::TimeWindowViolation { .. }));
    }
}
