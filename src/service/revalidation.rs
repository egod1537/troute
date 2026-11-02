use std::time::{Duration, Instant};

use crate::{
    api::TimeDependentRevalidationOutput,
    cancellation::CancellationToken,
    domain::{OptimizationProblem, RoutePlan, TimeOfDay},
    matrix::TravelTimeMatrix,
    routing::{RoutingContext, RoutingError, RoutingProvider},
    schedule::calculate_schedule_from,
    solver::SolverSolution,
};

use super::{check_cancelled, OptimizationServiceError, RouteOptimizationService};

const TIME_DEPENDENT_REVALIDATION_BUDGET: Duration = Duration::from_secs(30);
const TIME_DEPENDENT_WARNING_DELTA_MINUTES: u32 = 5;

pub(super) struct SelectedRouteRevalidation<'a> {
    pub(super) problem: &'a OptimizationProblem,
    pub(super) matrix: &'a TravelTimeMatrix,
    pub(super) solution: &'a SolverSolution,
    pub(super) selected_start_time: TimeOfDay,
    pub(super) static_plan: RoutePlan,
    pub(super) routing_context: &'a RoutingContext,
    pub(super) cancellation: &'a CancellationToken,
}

impl<P, S> RouteOptimizationService<P, S>
where
    P: RoutingProvider,
{
    pub(super) fn revalidate_selected_route(
        &self,
        request: SelectedRouteRevalidation<'_>,
    ) -> Result<(RoutePlan, Option<TimeDependentRevalidationOutput>), OptimizationServiceError>
    {
        let SelectedRouteRevalidation {
            problem,
            matrix,
            solution,
            selected_start_time,
            static_plan,
            routing_context,
            cancellation,
        } = request;
        let deadline = Instant::now()
            .checked_add(TIME_DEPENDENT_REVALIDATION_BUDGET)
            .unwrap_or_else(Instant::now);
        let mut rows = matrix.clone().into_rows();
        let mut revalidated_leg_count = 0;
        let mut changed_leg_count = 0;
        let mut max_delta_minutes = 0;

        for (leg_index, edge) in solution.visit_order.windows(2).enumerate() {
            check_cancelled(cancellation)?;
            let departure = static_plan
                .stops
                .get(leg_index)
                .and_then(|stop| stop.departure_time)
                .expect("every non-terminal scheduled stop has a departure time");
            let departure_time = match route_leg_departure_instant(routing_context, departure) {
                Ok(value) => value,
                Err(detail) => {
                    return Ok((
                        static_plan,
                        Some(TimeDependentRevalidationOutput {
                            attempted: true,
                            applied: false,
                            revalidated_leg_count,
                            changed_leg_count,
                            max_delta_minutes,
                            schedule_feasible: None,
                            warning_code: Some(
                                "TIME_DEPENDENT_REVALIDATION_UNAVAILABLE".to_owned(),
                            ),
                            warning_detail: Some(detail),
                        }),
                    ));
                }
            };
            let mut leg_context = routing_context.clone();
            leg_context.departure_time = Some(departure_time);
            let from = edge[0];
            let to = edge[1];
            let current = matrix
                .travel_minutes(from, to)
                .expect("the selected solution contains only reachable arcs");
            let updated = match self.matrix_service.travel_time_for_leg_with_control(
                &problem.locations()[from],
                &problem.locations()[to],
                &leg_context,
                cancellation,
                Some(deadline),
            ) {
                Ok(Some(minutes)) => minutes,
                Ok(None) => {
                    return Ok((
                        static_plan,
                        Some(TimeDependentRevalidationOutput {
                            attempted: true,
                            applied: false,
                            revalidated_leg_count,
                            changed_leg_count,
                            max_delta_minutes,
                            schedule_feasible: Some(false),
                            warning_code: Some(
                                "TIME_DEPENDENT_ROUTE_BECAME_UNREACHABLE".to_owned(),
                            ),
                            warning_detail: Some(format!(
                                "selected leg {} -> {} was unreachable at its actual departure time",
                                problem.locations()[from].id(),
                                problem.locations()[to].id()
                            )),
                        }),
                    ));
                }
                Err(RoutingError::Cancelled) => return Err(OptimizationServiceError::Cancelled),
                Err(error) => {
                    return Ok((
                        static_plan,
                        Some(TimeDependentRevalidationOutput {
                            attempted: true,
                            applied: false,
                            revalidated_leg_count,
                            changed_leg_count,
                            max_delta_minutes,
                            schedule_feasible: None,
                            warning_code: Some(
                                "TIME_DEPENDENT_REVALIDATION_UNAVAILABLE".to_owned(),
                            ),
                            warning_detail: Some(error.to_string()),
                        }),
                    ));
                }
            };
            revalidated_leg_count += 1;
            let delta = current.abs_diff(updated);
            max_delta_minutes = max_delta_minutes.max(delta);
            changed_leg_count += usize::from(current != updated);
            rows[from][to] = Some(updated);
        }

        let corrected = TravelTimeMatrix::new_sparse(rows)
            .expect("replacing selected arcs preserves matrix shape and diagonal");
        match calculate_schedule_from(problem, &corrected, solution, selected_start_time) {
            Ok(plan) => {
                let warning = (max_delta_minutes >= TIME_DEPENDENT_WARNING_DELTA_MINUTES).then(|| {
                    (
                        "TIME_DEPENDENT_TRAVEL_TIME_CHANGED".to_owned(),
                        format!(
                            "actual-departure revalidation changed {changed_leg_count} legs; maximum delta was {max_delta_minutes} minutes"
                        ),
                    )
                });
                Ok((
                    plan,
                    Some(TimeDependentRevalidationOutput {
                        attempted: true,
                        applied: true,
                        revalidated_leg_count,
                        changed_leg_count,
                        max_delta_minutes,
                        schedule_feasible: Some(true),
                        warning_code: warning.as_ref().map(|(code, _)| code.clone()),
                        warning_detail: warning.map(|(_, detail)| detail),
                    }),
                ))
            }
            Err(error) => Ok((
                static_plan,
                Some(TimeDependentRevalidationOutput {
                    attempted: true,
                    applied: false,
                    revalidated_leg_count,
                    changed_leg_count,
                    max_delta_minutes,
                    schedule_feasible: Some(false),
                    warning_code: Some("TIME_DEPENDENT_SCHEDULE_INFEASIBLE".to_owned()),
                    warning_detail: Some(error.to_string()),
                }),
            )),
        }
    }
}

fn route_leg_departure_instant(
    context: &RoutingContext,
    departure: TimeOfDay,
) -> Result<chrono::DateTime<chrono::Utc>, String> {
    use chrono::{LocalResult, NaiveDateTime, NaiveTime, TimeZone};

    let base = context
        .departure_time
        .ok_or_else(|| "dated revalidation requires the matrix departure instant".to_owned())?;
    let zone_name = context
        .timezone
        .as_deref()
        .ok_or_else(|| "dated revalidation requires an IANA time zone".to_owned())?;
    let zone = zone_name
        .parse::<chrono_tz::Tz>()
        .map_err(|_| format!("invalid IANA time zone during revalidation: {zone_name}"))?;
    let date = base.with_timezone(&zone).date_naive();
    let minute = departure.minutes();
    let time = NaiveTime::from_hms_opt(u32::from(minute / 60), u32::from(minute % 60), 0)
        .expect("validated TimeOfDay forms a local time");
    let local = NaiveDateTime::new(date, time);
    match zone.from_local_datetime(&local) {
        LocalResult::Single(value) => Ok(value.with_timezone(&chrono::Utc)),
        LocalResult::Ambiguous(_, _) => Err(format!(
            "actual leg departure {local} is ambiguous in {zone_name}"
        )),
        LocalResult::None => Err(format!(
            "actual leg departure {local} does not exist in {zone_name}"
        )),
    }
}
