use super::*;
use crate::{
    domain::Location,
    failure::{service_failure_detail, service_suggestions},
    matrix::TravelTimeMatrix,
    schedule::{visit_timing, ScheduleError},
    service::OptimizationServiceError,
};

/// Checks the submitted order without changing it or saving predicted times.
/// After an unreachable leg, suffix arrival times are unknown (never guessed).
pub fn validate_schedule(
    request: &ValidateRouteRequest,
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    source: &'static str,
) -> ValidateRouteResponse {
    let mut result = ValidateRouteResponse {
        input_valid: true,
        schedule_feasible: Some(true),
        blocks_optimization: false,
        matrix_source: source,
        issues: Vec::new(),
        stops: Vec::new(),
    };
    let mut current = Some(problem.start_time());
    let mut total_wait = 0_u32;
    for (index, location) in problem.locations().iter().enumerate() {
        if index > 0 {
            current = match matrix.travel_minutes(index - 1, index) {
                Some(minutes) => match current {
                    Some(time) => match time.checked_add(minutes) {
                        Some(arrival) => Some(arrival),
                        None => {
                            result.issues.push(schedule_issue(
                                request,
                                location,
                                ScheduleError::OutsideSingleDay,
                            ));
                            None
                        }
                    },
                    None => None,
                },
                None => {
                    result.issues.push(schedule_issue(
                        request,
                        location,
                        ScheduleError::UnreachableArc {
                            from_location_id: problem.locations()[index - 1].id().to_owned(),
                            to_location_id: location.id().to_owned(),
                        },
                    ));
                    None
                }
            };
        }
        let requested = request.service_start_times.get(location.id()).copied();
        let Some(arrival) = current else {
            result.stops.push(ValidationStop {
                location_id: location.id().to_owned(),
                arrival_time: None,
                service_start_time: None,
                departure_time: None,
                requested_service_start: requested,
            });
            continue;
        };
        let timing = visit_timing(location, arrival, requested);
        if let Some(fixed) = requested.filter(|fixed| *fixed < timing.service_start) {
            result.issues.push(ValidationIssue {
                code: "FIXED_VISIT_CONFLICT", severity: "warning", blocks_optimization: false,
                location_id: Some(location.id().to_owned()),
                message: "The requested visit time cannot be reached or is before opening time.".to_owned(),
                failure_detail: Some(serde_json::json!({"type": "FIXED_VISIT_CONFLICT", "location_id": location.id(), "requested_service_start": fixed, "earliest_service_start": timing.service_start, "arrival_time": arrival, "open_time": location.time_window().open(), "close_time": location.time_window().close()})), suggestions: Vec::new(),
            });
            // Keep the submitted time in the response; propagate the earliest
            // possible arrival rather than silently treating a late visit as on time.
        }
        total_wait = total_wait.saturating_add(timing.wait_minutes);
        match timing.departure {
            None => result.issues.push(schedule_issue(
                request,
                location,
                ScheduleError::OutsideSingleDay,
            )),
            Some(departure) if departure > location.time_window().close() => {
                result.issues.push(schedule_issue(
                    request,
                    location,
                    ScheduleError::TimeWindowViolation {
                        location_id: location.id().to_owned(),
                        arrival_time: arrival,
                        service_start_time: timing.service_start,
                        required_departure: departure,
                        close_time: location.time_window().close(),
                        selected_start_time: problem.start_time(),
                        total_wait_minutes_before_violation: total_wait,
                        is_start: index == 0,
                        is_destination: index + 1 == problem.locations().len(),
                        current_stay_minutes: location.stay_minutes(),
                        suggested_max_stay_minutes: u32::from(
                            location
                                .time_window()
                                .close()
                                .minutes()
                                .saturating_sub(timing.service_start.minutes()),
                        ),
                    },
                ));
            }
            _ => {}
        }
        result.stops.push(ValidationStop {
            location_id: location.id().to_owned(),
            arrival_time: Some(arrival),
            service_start_time: Some(timing.service_start),
            departure_time: timing.departure,
            requested_service_start: requested,
        });
        current = timing.departure;
    }
    result.schedule_feasible = Some(result.issues.is_empty());
    result
}

fn schedule_issue(
    request: &ValidateRouteRequest,
    location: &Location,
    error: ScheduleError,
) -> ValidationIssue {
    let code = match error {
        ScheduleError::UnreachableArc { .. } => "UNREACHABLE_ARC",
        ScheduleError::OutsideSingleDay => "OUTSIDE_SINGLE_DAY",
        _ => "TIME_WINDOW_VIOLATION",
    };
    let error = OptimizationServiceError::Schedule(error);
    ValidationIssue {
        code,
        severity: "warning",
        blocks_optimization: false,
        location_id: Some(location.id().to_owned()),
        message: error.to_string(),
        failure_detail: Some(serde_json::json!(service_failure_detail(
            &error,
            Some(&request.route)
        ))),
        suggestions: service_suggestions(&error, Some(&request.route)),
    }
}
