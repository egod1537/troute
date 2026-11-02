use super::*;
use crate::{
    api::OptimizeRouteRequest,
    cancellation::CancellationToken,
    domain::{Location, OptimizationProblem, RoutingReference, TimeOfDay, TimeWindow},
    matrix::TravelTimeMatrix,
};

fn problem(windows_and_stays: &[(&str, &str, u32)], start: &str) -> OptimizationProblem {
    let locations: Vec<_> = windows_and_stays
        .iter()
        .enumerate()
        .map(|(index, (open, close, stay))| {
            serde_json::json!({
                "id": index.to_string(),
                "place_id": format!("place-{index}"),
                "open_time": open,
                "close_time": close,
                "stay_minutes": stay
            })
        })
        .collect();
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "exact-solver-test",
        "locations": locations,
        "start_time": start
    }))
    .unwrap();
    request.try_into().unwrap()
}

fn detailed(
    problem: &OptimizationProblem,
    rows: Vec<Vec<u32>>,
) -> Result<ExactSolveResult, SolverError> {
    let matrix = TravelTimeMatrix::new(rows).unwrap();
    ExactBitDpSolver::default().solve_detailed(SolverInput {
        matrix: &matrix,
        problem,
        cancellation: &CancellationToken::new(),
    })
}

#[test]
fn solves_one_location() {
    let problem = OptimizationProblem::new(
        vec![Location::new(
            "0".to_owned(),
            RoutingReference::GooglePlaceId("place-0".to_owned()),
            TimeWindow::new(
                TimeOfDay::from_minutes(0).unwrap(),
                TimeOfDay::from_minutes(1439).unwrap(),
            )
            .unwrap(),
            0,
        )],
        TimeOfDay::from_minutes(540).unwrap(),
    );
    let result = detailed(&problem, vec![vec![0]]).unwrap();
    assert_eq!(result.solution.visit_order, vec![0]);
    assert_eq!(result.metrics.start_time_slot, 1439);
    assert_eq!(result.metrics.finish_time_slot, 1439);
}

#[test]
fn solves_three_locations_and_respects_opening_and_closing() {
    let problem = problem(
        &[
            ("00:00", "23:50", 0),
            ("10:00", "10:30", 20),
            ("00:00", "23:50", 0),
        ],
        "09:00",
    );
    let result = detailed(
        &problem,
        vec![vec![0, 10, 90], vec![10, 0, 10], vec![90, 10, 0]],
    )
    .unwrap();
    assert_eq!(result.solution.visit_order, vec![0, 1, 2]);
    assert_eq!(result.metrics.start_time_slot, 10 * 60);
    assert_eq!(result.metrics.finish_time_slot, 10 * 60 + 40);
}

#[test]
fn records_waiting_at_a_fixed_start_slot() {
    let problem = problem(
        &[
            ("00:00", "23:50", 0),
            ("10:00", "18:00", 10),
            ("00:00", "23:50", 0),
        ],
        "09:00",
    );
    let matrix = TravelTimeMatrix::new(vec![vec![0; 3]; 3]).unwrap();
    let cancellation = CancellationToken::new();
    let input = SolverInput {
        matrix: &matrix,
        problem: &problem,
        cancellation: &cancellation,
    };
    let result = ExactBitDpSolver::default()
        .search_at_start(&input, 9 * 60)
        .unwrap()
        .unwrap();
    assert_eq!(result.metrics.wait_minutes, 60);
}

#[test]
fn reports_infeasible_time_windows() {
    let problem = problem(
        &[
            ("00:00", "23:50", 0),
            ("09:00", "09:10", 20),
            ("00:00", "23:50", 0),
        ],
        "09:00",
    );
    assert!(matches!(
        detailed(&problem, vec![vec![0; 3]; 3]),
        Err(SolverError::NoFeasibleRoute)
    ));
}

#[test]
fn directed_times_choose_the_only_feasible_order() {
    let problem = problem(
        &[
            ("00:00", "23:50", 0),
            ("09:00", "12:00", 0),
            ("09:00", "12:00", 0),
            ("00:00", "23:50", 0),
        ],
        "09:00",
    );
    let huge = 240;
    let result = detailed(
        &problem,
        vec![
            vec![0, 10, huge, 0],
            vec![huge, 0, 10, 10],
            vec![huge, huge, 0, 10],
            vec![0, 0, 0, 0],
        ],
    )
    .unwrap();
    assert_eq!(result.solution.visit_order, vec![0, 1, 2, 3]);
}

#[test]
fn latest_start_precedes_cost_and_finish_breaks_ties() {
    let problem = problem(
        &[
            ("00:00", "23:50", 0),
            ("09:00", "12:00", 0),
            ("09:00", "12:00", 0),
            ("00:00", "23:50", 0),
        ],
        "09:00",
    );
    let rows = vec![
        vec![0, 10, 10, 0],
        vec![0, 0, 20, 10],
        vec![0, 20, 0, 40],
        vec![0, 0, 0, 0],
    ];
    let result = detailed(&problem, rows.clone()).unwrap();
    assert_eq!(result.metrics.start_time_slot, 11 * 60 + 30);
    assert_eq!(result.solution.visit_order, vec![0, 2, 1, 3]);
    assert_eq!(result.metrics.finish_time_slot, 12 * 60 + 10);

    let matrix = TravelTimeMatrix::new(rows).unwrap();
    let selected = ExactBitDpSolver::default()
        .selected_start_time(
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            },
            &result.solution,
        )
        .unwrap();
    assert_eq!(selected.to_string(), "11:30");
}

#[test]
fn latest_visit_start_moves_back_by_the_start_stay() {
    let without_start_stay = problem(&[("00:00", "23:50", 0), ("00:00", "11:00", 0)], "00:00");
    let with_start_stay = problem(&[("00:00", "23:50", 60), ("00:00", "11:00", 0)], "00:00");
    let rows = vec![vec![0, 30], vec![30, 0]];

    let without = detailed(&without_start_stay, rows.clone()).unwrap();
    let with = detailed(&with_start_stay, rows).unwrap();

    assert_eq!(without.metrics.start_time_slot, 10 * 60 + 30);
    assert_eq!(with.metrics.start_time_slot, 9 * 60 + 30);
    assert_eq!(
        without.metrics.start_time_slot - with.metrics.start_time_slot,
        60
    );
    assert_eq!(with.metrics.finish_time_slot, 11 * 60);
}

#[test]
fn pareto_pruning_retains_the_optimal_predecessor_chain() {
    let problem = problem(&[("00:00", "23:50", 0); 5], "09:00");
    let mut rows = vec![vec![600; 5]; 5];
    for (from, to, minutes) in [(0, 1, 10), (1, 2, 10), (2, 3, 10), (3, 4, 10)] {
        rows[from][to] = minutes;
    }
    // A slower path reaches the same (mask, last=3) frontier cell.
    for (from, to, minutes) in [(0, 2, 40), (2, 1, 40), (1, 3, 40)] {
        rows[from][to] = minutes;
    }
    for (index, row) in rows.iter_mut().enumerate() {
        row[index] = 0;
    }
    let matrix = TravelTimeMatrix::new(rows).unwrap();
    let cancellation = CancellationToken::new();
    let result = ExactBitDpSolver::default()
        .search_at_start(
            &SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &cancellation,
            },
            54,
        )
        .unwrap()
        .unwrap();
    assert_eq!(result.solution.visit_order, vec![0, 1, 2, 3, 4]);
    assert_eq!(result.metrics.travel_minutes, 40);
}

#[test]
fn rejects_more_than_the_exact_limit() {
    let entries = vec![("00:00", "23:50", 0); EXACT_MAX_LOCATIONS + 1];
    let problem = problem(&entries, "09:00");
    let size = entries.len();
    assert!(matches!(
        detailed(&problem, vec![vec![0; size]; size]),
        Err(SolverError::UnsupportedLocationCount {
            maximum: EXACT_MAX_LOCATIONS,
            actual
        }) if actual == size
    ));
}
