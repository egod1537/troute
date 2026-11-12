use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::{json, Value};
use troute::{
    api::OptimizeRouteRequest,
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    schedule::calculate_schedule,
    solver::{
        check_route_feasibility, FeasibilityMethod, SolverError, SolverInput, SolverSolution,
    },
    CancellationToken,
};

fn route(n: usize) -> Value {
    json!({
        "job_id": "feasibility", "start_policy": "FIXED", "start_time": "09:00",
        "locations": (0..n).map(|i| json!({
            "id": format!("p{i}"), "place_id": format!("p{i}"),
            "open_time": "09:00", "close_time": "18:00", "stay_minutes": 0
        })).collect::<Vec<_>>()
    })
}

fn problem(value: Value) -> OptimizationProblem {
    OptimizationProblem::try_from(serde_json::from_value::<OptimizeRouteRequest>(value).unwrap())
        .unwrap()
}

fn check(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
) -> Result<troute::solver::FeasibilitySolution, SolverError> {
    check_route_feasibility(SolverInput {
        problem,
        matrix,
        cancellation: &CancellationToken::new(),
    })
}

fn trap() -> (Value, TravelTimeMatrix) {
    let mut value = route(4);
    value["locations"][1]["stay_minutes"] = json!(20);
    value["locations"][2]["stay_minutes"] = json!(20);
    value["locations"][2]["close_time"] = json!("09:40");
    let matrix = TravelTimeMatrix::new(vec![
        vec![0, 1, 10, 50],
        vec![1, 0, 20, 5],
        vec![10, 10, 0, 5],
        vec![50, 5, 5, 0],
    ])
    .unwrap();
    (value, matrix)
}

#[test]
fn greedy_success_returns_a_valid_witness_and_stops_before_dp() {
    let p = problem(route(4));
    let m = TravelTimeMatrix::new(vec![
        vec![0, 1, 2, 3],
        vec![1, 0, 1, 2],
        vec![2, 1, 0, 1],
        vec![3, 2, 1, 0],
    ])
    .unwrap();
    let found = check(&p, &m).unwrap();
    assert_eq!(found.method, FeasibilityMethod::Greedy);
    calculate_schedule(&p, &m, &found.solution).unwrap();
}

#[test]
fn failed_greedy_falls_back_to_dp_instead_of_claiming_infeasibility() {
    let (value, matrix) = trap();
    let p = problem(value);
    let found = check(&p, &matrix).unwrap();
    assert_eq!(found.method, FeasibilityMethod::BitDp);
    assert_eq!(found.solution.visit_order, [0, 2, 1, 3]);
    calculate_schedule(&p, &matrix, &found.solution).unwrap();
}

#[test]
fn dp_can_prove_infeasibility_and_respects_locked_positions() {
    let (mut value, matrix) = trap();
    value["locations"][1]["order_locked"] = json!(true);
    assert!(matches!(
        check(&problem(value), &matrix),
        Err(SolverError::ProvenInfeasible)
    ));
}

#[test]
fn sparse_directed_arcs_can_have_a_witness_even_when_the_greedy_order_fails() {
    let p = problem(route(4));
    let matrix = TravelTimeMatrix::new_sparse(vec![
        vec![Some(0), Some(1), Some(2), None],
        vec![None, Some(0), None, Some(1)],
        vec![None, Some(1), Some(0), None],
        vec![None, None, None, Some(0)],
    ])
    .unwrap();
    let found = check(&p, &matrix).unwrap();
    assert_eq!(found.method, FeasibilityMethod::BitDp);
    assert_eq!(found.solution.visit_order, [0, 2, 1, 3]);
}

#[test]
fn best_effort_can_skip_an_impossible_optional_location() {
    let (mut value, matrix) = trap();
    value["visit_policy"] = json!("BEST_EFFORT_ON_FAILURE");
    value["locations"][1]["required"] = json!(false);
    value["locations"][2]["required"] = json!(true);
    value["locations"][1]["stay_minutes"] = json!(600);
    let p = problem(value);
    let found = check(&p, &matrix).unwrap();
    assert_eq!(found.method, FeasibilityMethod::BitDp);
    assert_eq!(found.solution.visit_order, [0, 2, 3]);
    calculate_schedule(&p, &matrix, &found.solution).unwrap();
}

#[test]
fn fifteen_is_supported_but_sixteen_is_rejected_even_when_greedy_would_succeed() {
    for n in [15, 16] {
        let p = problem(route(n));
        let m = TravelTimeMatrix::new(vec![vec![0; n]; n]).unwrap();
        match check(&p, &m) {
            Ok(found) => assert_eq!((n, found.solution.visit_order.len()), (15, 15)),
            Err(SolverError::UnsupportedLocationCount { maximum, actual }) => {
                assert_eq!((maximum, actual), (15, 16))
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
}

#[test]
fn cancellation_is_an_error_rather_than_an_infeasibility_proof() {
    let p = problem(route(2));
    let m = TravelTimeMatrix::new(vec![vec![0, 1], vec![1, 0]]).unwrap();
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        check_route_feasibility(SolverInput {
            problem: &p,
            matrix: &m,
            cancellation: &cancellation
        }),
        Err(SolverError::Cancelled)
    ));
}

fn exhaustive(
    problem: &OptimizationProblem,
    matrix: &TravelTimeMatrix,
    order: &mut Vec<usize>,
    remaining: &mut Vec<usize>,
) -> bool {
    if remaining.is_empty() {
        order.push(problem.end_location_index());
        let feasible = calculate_schedule(
            problem,
            matrix,
            &SolverSolution {
                visit_order: order.clone(),
            },
        )
        .is_ok();
        order.pop();
        return feasible;
    }
    for i in 0..remaining.len() {
        let next = remaining.remove(i);
        order.push(next);
        let found = exhaustive(problem, matrix, order, remaining);
        order.pop();
        remaining.insert(i, next);
        if found {
            return true;
        }
    }
    false
}

#[test]
fn existence_matches_exhaustive_schedule_validation_on_varied_small_problems() {
    let mut rng = StdRng::seed_from_u64(412);
    let mut feasible_count = 0;
    for iteration in 0..120 {
        let mut value = route(6);
        value["start_policy"] = json!(["FIXED", "EARLIEST", "LATEST"][iteration % 3]);
        for i in 1..5 {
            let open = 9 * 60 + 10 * rng.gen_range(0..8);
            let close = open + 10 * rng.gen_range(1..9);
            value["locations"][i]["open_time"] =
                json!(format!("{:02}:{:02}", open / 60, open % 60));
            value["locations"][i]["close_time"] =
                json!(format!("{:02}:{:02}", close / 60, close % 60));
            value["locations"][i]["stay_minutes"] = json!(10 * rng.gen_range(0..4));
            value["locations"][i]["order_locked"] = json!(rng.gen_bool(0.2));
        }
        let p = problem(value);
        let rows = (0..6)
            .map(|i| {
                (0..6)
                    .map(|j| if i == j { 0 } else { rng.gen_range(1..31) })
                    .collect()
            })
            .collect();
        let matrix = TravelTimeMatrix::new(rows).unwrap();
        let expected = exhaustive(&p, &matrix, &mut vec![0], &mut (1..5).collect());
        let actual = match check(&p, &matrix) {
            Ok(found) => {
                calculate_schedule(&p, &matrix, &found.solution).unwrap();
                true
            }
            Err(SolverError::ProvenInfeasible) => false,
            other => panic!("unexpected result: {other:?}"),
        };
        assert_eq!(actual, expected, "case {iteration}");
        feasible_count += usize::from(actual);
    }
    assert!(feasible_count > 0 && feasible_count < 120);
}
