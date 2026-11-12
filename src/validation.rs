//! Read-only current-order validation and bounded itinerary existence checks.
use crate::{
    api::OptimizeRouteRequest,
    domain::{OptimizationProblem, TimeOfDay},
    failure::FailureSuggestion,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
mod evaluate;
mod feasibility;
pub use evaluate::validate_schedule;
pub(crate) use feasibility::validate_feasibility;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidateRouteRequest {
    pub route: OptimizeRouteRequest,
    #[serde(default)]
    pub check_feasibility: bool,
    #[serde(default)]
    pub service_start_times: HashMap<String, TimeOfDay>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationIssue {
    pub code: &'static str,
    pub severity: &'static str,
    pub blocks_optimization: bool,
    pub location_id: Option<String>,
    pub message: String,
    pub failure_detail: Option<serde_json::Value>,
    pub suggestions: Vec<FailureSuggestion>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidationStop {
    pub location_id: String,
    pub arrival_time: Option<TimeOfDay>,
    pub service_start_time: Option<TimeOfDay>,
    pub departure_time: Option<TimeOfDay>,
    pub requested_service_start: Option<TimeOfDay>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidateRouteResponse {
    pub input_valid: bool,
    pub schedule_feasible: Option<bool>,
    pub blocks_optimization: bool,
    pub matrix_source: &'static str,
    pub issues: Vec<ValidationIssue>,
    pub stops: Vec<ValidationStop>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feasibility: Option<FeasibilityValidation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FeasibilityStatus {
    Feasible,
    Infeasible,
    Unsupported,
}

#[derive(Debug, Clone, Serialize)]
pub struct FeasibilityValidation {
    pub status: FeasibilityStatus,
    pub method: Option<crate::solver::FeasibilityMethod>,
    pub location_count: usize,
    pub max_locations: usize,
    pub visit_order: Vec<String>,
}

impl ValidateRouteResponse {
    /// Check the cap before matrix/provider work or a greedy attempt.
    pub(crate) fn unsupported(request: &ValidateRouteRequest) -> Option<Self> {
        let count = request.route.locations.len();
        (request.check_feasibility && count > crate::solver::EXACT_MAX_LOCATIONS).then(|| Self {
            input_valid: true,
            schedule_feasible: None,
            blocks_optimization: false,
            matrix_source: "none",
            issues: Vec::new(),
            stops: Vec::new(),
            feasibility: Some(FeasibilityValidation {
                status: FeasibilityStatus::Unsupported,
                method: None,
                location_count: count,
                max_locations: crate::solver::EXACT_MAX_LOCATIONS,
                visit_order: Vec::new(),
            }),
        })
    }

    pub(crate) fn invalid(message: String) -> Self {
        Self {
            input_valid: false,
            schedule_feasible: None,
            blocks_optimization: true,
            matrix_source: "none",
            stops: Vec::new(),
            feasibility: None,
            issues: vec![ValidationIssue {
                code: "INVALID_REQUEST",
                severity: "error",
                blocks_optimization: true,
                location_id: None,
                message,
                failure_detail: Some(serde_json::json!({"type": "INVALID_REQUEST"})),
                suggestions: Vec::new(),
            }],
        }
    }
}

/// Shares request/domain validation with optimization, before any routing I/O.
pub(crate) fn validate_input(
    request: &ValidateRouteRequest,
) -> Result<OptimizationProblem, ValidateRouteResponse> {
    if request.check_feasibility && !request.service_start_times.is_empty() {
        return Err(ValidateRouteResponse::invalid(
            "service_start_times is only supported for current-order validation".to_owned(),
        ));
    }
    let problem = OptimizationProblem::try_from(request.route.clone())
        .map_err(|error| ValidateRouteResponse::invalid(error.to_string()))?;
    if request.service_start_times.keys().any(|id| {
        !request
            .route
            .locations
            .iter()
            .any(|location| &location.id == id)
    }) {
        return Err(ValidateRouteResponse::invalid(
            "service_start_times contains a location outside this route".to_owned(),
        ));
    }
    Ok(problem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matrix::TravelTimeMatrix;

    fn request() -> ValidateRouteRequest {
        serde_json::from_value(serde_json::json!({
            "route": {"job_id": "read-only-validation", "start_policy": "FIXED", "start_time": "09:00",
                "locations": [
                    {"id": "a", "place_id": "a", "open_time": "09:00", "close_time": "10:00", "stay_minutes": 20},
                    {"id": "b", "place_id": "b", "open_time": "09:00", "close_time": "10:00", "stay_minutes": 20},
                    {"id": "c", "place_id": "c", "open_time": "09:00", "close_time": "10:00", "stay_minutes": 20}
                ], "travel_time_matrix": [[0,10,10],[10,0,10],[10,10,0]]},
            "service_start_times": {}
        })).unwrap()
    }

    #[test]
    fn current_order_and_pinned_times_are_preserved_while_all_issues_are_reported() {
        let mut request = request();
        request
            .service_start_times
            .insert("b".to_owned(), "09:10".parse().unwrap());
        request
            .service_start_times
            .insert("c".to_owned(), "09:40".parse().unwrap());
        let before = serde_json::to_value(&request.route).unwrap();
        let problem = validate_input(&request).unwrap();
        let matrix =
            TravelTimeMatrix::new(request.route.travel_time_matrix.clone().unwrap()).unwrap();
        let result = validate_schedule(&request, &problem, &matrix, "supplied");
        assert!(result.input_valid);
        assert!(!result.blocks_optimization);
        assert_eq!(result.schedule_feasible, Some(false));
        assert_eq!(
            result
                .issues
                .iter()
                .filter(|issue| issue.code == "FIXED_VISIT_CONFLICT")
                .count(),
            2
        );
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.code == "TIME_WINDOW_VIOLATION"));
        assert_eq!(
            result
                .stops
                .iter()
                .map(|stop| stop.location_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
        assert_eq!(
            result.stops[1].requested_service_start,
            Some("09:10".parse().unwrap())
        );
        assert_eq!(serde_json::to_value(&request.route).unwrap(), before);
    }

    #[test]
    fn reachable_feasible_order_shares_optimization_schedule_semantics() {
        let mut request = request();
        request.route.locations[2].close_time = "11:00".parse().unwrap();
        let problem = validate_input(&request).unwrap();
        let matrix =
            TravelTimeMatrix::new(request.route.travel_time_matrix.clone().unwrap()).unwrap();
        let result = validate_schedule(&request, &problem, &matrix, "supplied");
        let plan = crate::schedule::calculate_schedule(
            &problem,
            &matrix,
            &crate::solver::SolverSolution {
                visit_order: vec![0, 1, 2],
            },
        )
        .unwrap();
        assert_eq!(result.schedule_feasible, Some(true));
        for (actual, expected) in result.stops.iter().zip(plan.stops) {
            assert_eq!(actual.arrival_time, Some(expected.arrival_time));
            assert_eq!(actual.service_start_time, Some(expected.service_start_time));
            assert_eq!(actual.departure_time, expected.departure_time);
        }
    }

    #[test]
    fn unreachable_leg_never_fabricates_suffix_arrivals() {
        let request = request();
        let problem = validate_input(&request).unwrap();
        let matrix = TravelTimeMatrix::new_sparse(vec![
            vec![Some(0), None, Some(10)],
            vec![Some(10), Some(0), Some(10)],
            vec![Some(10), Some(10), Some(0)],
        ])
        .unwrap();
        let result = validate_schedule(&request, &problem, &matrix, "supplied");
        assert_eq!(result.issues[0].code, "UNREACHABLE_ARC");
        assert!(result.stops[1].arrival_time.is_none());
        assert!(result.stops[2].arrival_time.is_none());
        assert!(!result.blocks_optimization);
    }

    #[test]
    fn malformed_domain_input_blocks_optimization_without_routing() {
        let mut request = request();
        request.route.locations[1].open_time = "12:00".parse().unwrap();
        let result = validate_input(&request).unwrap_err();
        assert!(result.blocks_optimization);
        assert!(!result.input_valid);
        assert_eq!(result.schedule_feasible, None);
    }
}
