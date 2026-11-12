use super::*;
use crate::{api::OptimizeRouteRequest, solver::check_route_feasibility};
use serde_json::{json, Value};

fn request(n: usize) -> Value {
    json!({"job_id":"repair", "start_policy":"FIXED", "start_time":"09:00",
        "locations":(0..n).map(|i| json!({"id":format!("p{i}"),"place_id":format!("p{i}"),"open_time":"09:00","close_time":"18:00","stay_minutes":0})).collect::<Vec<_>>()})
}

fn check(value: Value, matrix: &TravelTimeMatrix) -> (OptimizationProblem, RecoveryResult) {
    let problem = OptimizationProblem::try_from(
        serde_json::from_value::<OptimizeRouteRequest>(value).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        check_route_feasibility(SolverInput {
            problem: &problem,
            matrix,
            cancellation: &CancellationToken::new()
        }),
        Err(SolverError::ProvenInfeasible)
    ));
    let result = find_recovery_suggestions(
        &problem,
        matrix,
        &CancellationToken::new(),
        RemediationConfig::default(),
    );
    // Independently apply each advertised change and run the ordinary checker.
    for suggestion in &result.suggestions {
        assert!(suggestion.feasibility_verified);
        let mut locations = problem.locations().to_vec();
        let mut start = problem.start_time();
        let mut updated_matrix = matrix.clone();
        match suggestion.suggestion_type {
            FailureSuggestionType::StartEarlier => {
                start = suggestion.suggested_latest_start.unwrap()
            }
            FailureSuggestionType::ReduceStayTime => {
                let index = locations
                    .iter()
                    .position(|l| Some(l.id()) == suggestion.location_id.as_deref())
                    .unwrap();
                locations[index] = locations[index]
                    .clone()
                    .with_stay_minutes(suggestion.suggested_max_minutes.unwrap());
            }
            FailureSuggestionType::RemoveLocation => {
                let index = locations
                    .iter()
                    .position(|l| Some(l.id()) == suggestion.location_id.as_deref())
                    .unwrap();
                locations.remove(index);
                let indices: Vec<_> = (0..matrix.size()).filter(|&i| i != index).collect();
                updated_matrix = TravelTimeMatrix::new_sparse(
                    indices
                        .iter()
                        .map(|&i| {
                            indices
                                .iter()
                                .map(|&j| matrix.travel_minutes(i, j))
                                .collect()
                        })
                        .collect(),
                )
                .unwrap();
            }
            _ => panic!("unexpected suggestion"),
        }
        let changed = OptimizationProblem::with_policies(
            locations,
            start,
            problem.start_policy(),
            problem.visit_policy(),
        );
        check_route_feasibility(SolverInput {
            problem: &changed,
            matrix: &updated_matrix,
            cancellation: &CancellationToken::new(),
        })
        .unwrap();
    }
    (problem, result)
}

#[test]
fn stay_repair_is_a_ten_minute_change_that_repairs_the_entire_route() {
    let mut value = request(3);
    value["locations"][1]["stay_minutes"] = json!(60);
    value["locations"][1]["close_time"] = json!("09:40");
    let (_, result) = check(value, &TravelTimeMatrix::new(vec![vec![0; 3]; 3]).unwrap());
    let stay = result
        .suggestions
        .iter()
        .find(|s| s.suggestion_type == FailureSuggestionType::ReduceStayTime)
        .unwrap();
    assert_eq!(stay.location_id.as_deref(), Some("p1"));
    assert_eq!(stay.current_minutes, Some(60));
    assert_eq!(stay.suggested_max_minutes, Some(40));
}

#[test]
fn earlier_start_repairs_fixed_start_without_changing_places_or_windows() {
    let mut value = request(2);
    value["start_time"] = json!("12:00");
    value["locations"][1]["close_time"] = json!("12:00");
    let (_, result) = check(
        value,
        &TravelTimeMatrix::new(vec![vec![0, 30], vec![30, 0]]).unwrap(),
    );
    assert_eq!(result.suggestions.len(), 1);
    assert_eq!(
        result.suggestions[0].suggested_latest_start,
        Some("11:30".parse().unwrap())
    );
}

#[test]
fn removal_is_verified_and_does_not_delete_an_endpoint_or_shift_a_pin() {
    let mut value = request(4);
    value["locations"][1]["stay_minutes"] = json!(10);
    value["locations"][1]["close_time"] = json!("09:00");
    let matrix = TravelTimeMatrix::new(vec![vec![0; 4]; 4]).unwrap();
    let (_, result) = check(value.clone(), &matrix);
    assert_eq!(result.suggestions.len(), 1);
    assert_eq!(
        result.suggestions[0].suggestion_type,
        FailureSuggestionType::RemoveLocation
    );
    assert_eq!(result.suggestions[0].location_id.as_deref(), Some("p1"));
    value["locations"][2]["order_locked"] = json!(true);
    assert!(check(value, &matrix).1.suggestions.is_empty());
}

#[test]
fn local_fixes_that_leave_another_failure_are_not_advertised() {
    let mut value = request(4);
    for i in [1, 2] {
        value["locations"][i]["stay_minutes"] = json!(10);
        value["locations"][i]["close_time"] = json!("09:00");
    }
    assert!(
        check(value, &TravelTimeMatrix::new(vec![vec![0; 4]; 4]).unwrap())
            .1
            .suggestions
            .is_empty()
    );
}

#[test]
fn exhausted_budget_keeps_the_original_infeasibility_and_returns_no_guesses() {
    let problem = OptimizationProblem::try_from(
        serde_json::from_value::<OptimizeRouteRequest>(request(2)).unwrap(),
    )
    .unwrap();
    let result = find_recovery_suggestions(
        &problem,
        &TravelTimeMatrix::new(vec![vec![0, 1], vec![1, 0]]).unwrap(),
        &CancellationToken::new(),
        RemediationConfig {
            budget: Duration::ZERO,
            ..RemediationConfig::default()
        },
    );
    assert_eq!(result.probes, 0);
    assert!(result.budget_exhausted && result.suggestions.is_empty());
    assert_eq!(
        largest_feasible(0, 10, |step| if step == 0 { Some(true) } else { None }),
        Some(0)
    );
}
