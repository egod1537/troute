use super::{
    FeasibilityStatus, FeasibilityValidation, ValidateRouteResponse, ValidationIssue,
    ValidationStop,
};
use crate::{
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    schedule::calculate_schedule,
    service::OptimizationServiceError,
    solver::{
        check_route_feasibility, FeasibilityMethod, SolverError, SolverInput, EXACT_MAX_LOCATIONS,
    },
    CancellationToken,
};

pub(crate) fn validate_feasibility(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    source: &'static str,
    cancellation: &CancellationToken,
    remediation: crate::failure::RemediationConfig,
) -> Result<ValidateRouteResponse, OptimizationServiceError> {
    let checked = check_route_feasibility(SolverInput {
        problem,
        matrix,
        cancellation,
    });
    let mut result = ValidateRouteResponse {
        input_valid: true,
        schedule_feasible: Some(false),
        blocks_optimization: false,
        matrix_source: source,
        issues: Vec::new(),
        stops: Vec::new(),
        feasibility: Some(FeasibilityValidation {
            status: FeasibilityStatus::Infeasible,
            method: Some(FeasibilityMethod::BitDp),
            location_count: problem.locations().len(),
            max_locations: EXACT_MAX_LOCATIONS,
            visit_order: Vec::new(),
        }),
    };
    let found = match checked {
        Ok(found) => found,
        Err(SolverError::ProvenInfeasible) => {
            let recovery = super::recovery::find_recovery_suggestions(
                problem,
                matrix,
                cancellation,
                remediation,
            );
            if cancellation.is_cancelled() {
                return Err(OptimizationServiceError::Solver(SolverError::Cancelled));
            }
            result.issues.push(ValidationIssue {
                code: "NO_FEASIBLE_ROUTE",
                severity: "warning",
                blocks_optimization: false,
                location_id: None,
                message: "No visit order satisfies the supplied itinerary constraints.".to_owned(),
                failure_detail: Some(serde_json::json!({"type":"NO_FEASIBLE_ROUTE", "remediation": {"probes": recovery.probes, "budget_exhausted": recovery.budget_exhausted}})),
                suggestions: recovery.suggestions,
            });
            return Ok(result);
        }
        Err(error) => return Err(OptimizationServiceError::Solver(error)),
    };
    let plan = calculate_schedule(problem, matrix, &found.solution)?;
    result.schedule_feasible = Some(true);
    let feasibility = result
        .feasibility
        .as_mut()
        .expect("existence check metadata is present");
    feasibility.status = FeasibilityStatus::Feasible;
    feasibility.method = Some(found.method);
    feasibility.visit_order = found
        .solution
        .visit_order
        .iter()
        .map(|&i| problem.locations()[i].id().to_owned())
        .collect();
    result.stops = plan
        .stops
        .into_iter()
        .map(|stop| ValidationStop {
            location_id: problem.locations()[stop.location_index].id().to_owned(),
            arrival_time: Some(stop.arrival_time),
            service_start_time: Some(stop.service_start_time),
            departure_time: stop.departure_time,
            requested_service_start: None,
        })
        .collect();
    Ok(result)
}
