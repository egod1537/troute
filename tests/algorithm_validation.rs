use std::{cmp::Ordering, collections::BTreeSet};

use rand::{rngs::StdRng, Rng, SeedableRng};
use troute::{
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    schedule::calculate_schedule_from,
    solver::{
        evaluate_solution, BitDpPerfectMatching, BlossomPerfectMatching, ChristofidesInitialRoute,
        DefaultObjectivePolicy, ExactBitDpSolver, FrontierPoint, FrontierPolicy,
        GreedyInitialRoute, InitialRouteGenerator, MstDoubleTreeInitialRoute,
        PerfectMatchingStrategy, SolutionMetrics, SolverError, SolverInput, SolverSolution,
        TimeCostFrontierPolicy, TIME_SLOT_MINUTES,
    },
    OptimizeRouteRequest,
};

const SLOTS_PER_DAY: u32 = 24 * 60 / TIME_SLOT_MINUTES;
const RANDOM_SEEDS: [u64; 5] = [1, 7, 42, 1_337, 0x5eed];

struct GeneratedCase {
    problem: OptimizationProblem,
    matrix: TravelTimeMatrix,
}

/// A deliberately independent exhaustive oracle. It does not call the shared
/// route evaluator, so a transition/evaluation regression cannot make both
/// sides of the comparison pass in the same way.
fn brute_force(case: &GeneratedCase) -> Option<(SolverSolution, SolutionMetrics)> {
    let count = case.problem.locations().len();
    let mut intermediates: Vec<_> = (1..count.saturating_sub(1)).collect();
    let mut best = None;
    loop {
        let mut order = Vec::with_capacity(count);
        order.push(0);
        order.extend(intermediates.iter().copied());
        if count > 1 {
            order.push(count - 1);
        }
        let solution = SolverSolution { visit_order: order };
        if let Some(metrics) = independently_evaluate(case, &solution) {
            let improves = best
                .as_ref()
                .map(|(_, current)| compare_metrics(&metrics, current).is_lt())
                .unwrap_or(true);
            if improves {
                best = Some((solution, metrics));
            }
        }
        if !next_permutation(&mut intermediates) {
            break;
        }
    }
    best
}

fn independently_evaluate(
    case: &GeneratedCase,
    solution: &SolverSolution,
) -> Option<SolutionMetrics> {
    let earliest = u32::from(case.problem.start_time().minutes()).div_ceil(TIME_SLOT_MINUTES);
    for start in (earliest..SLOTS_PER_DAY).rev() {
        if let Some(metrics) = independently_simulate(case, solution, start as u16) {
            return Some(metrics);
        }
    }
    None
}

fn independently_simulate(
    case: &GeneratedCase,
    solution: &SolverSolution,
    start: u16,
) -> Option<SolutionMetrics> {
    let mut slot = u32::from(start);
    let mut travel_minutes = 0_u32;
    let mut wait_minutes = 0_u32;
    for edge in solution.visit_order.windows(2) {
        let travel = case.matrix.travel_minutes(edge[0], edge[1])?;
        travel_minutes = travel_minutes.checked_add(travel)?;
        let arrival = slot.checked_add(travel.div_ceil(TIME_SLOT_MINUTES))?;
        let location = &case.problem.locations()[edge[1]];
        let open = u32::from(location.time_window().open().minutes()).div_ceil(TIME_SLOT_MINUTES);
        let service_start = arrival.max(open);
        wait_minutes =
            wait_minutes.checked_add((service_start - arrival).checked_mul(TIME_SLOT_MINUTES)?)?;
        let finish =
            service_start.checked_add(location.stay_minutes().div_ceil(TIME_SLOT_MINUTES))?;
        let close = u32::from(location.time_window().close().minutes()) / TIME_SLOT_MINUTES;
        if finish > close || finish >= SLOTS_PER_DAY {
            return None;
        }
        slot = finish;
    }
    Some(SolutionMetrics {
        start_time_slot: start,
        finish_time_slot: slot as u16,
        travel_minutes,
        wait_minutes,
        score: u64::from(travel_minutes) + u64::from(wait_minutes),
    })
}

fn compare_metrics(left: &SolutionMetrics, right: &SolutionMetrics) -> Ordering {
    right
        .start_time_slot
        .cmp(&left.start_time_slot)
        .then_with(|| left.finish_time_slot.cmp(&right.finish_time_slot))
        .then_with(|| left.travel_minutes.cmp(&right.travel_minutes))
        .then_with(|| left.wait_minutes.cmp(&right.wait_minutes))
        .then_with(|| left.score.cmp(&right.score))
}

fn next_permutation(values: &mut [usize]) -> bool {
    let Some(pivot) = (0..values.len().saturating_sub(1))
        .rev()
        .find(|&index| values[index] < values[index + 1])
    else {
        return false;
    };
    let successor = (pivot + 1..values.len())
        .rev()
        .find(|&index| values[pivot] < values[index])
        .expect("a permutation successor exists after the pivot");
    values.swap(pivot, successor);
    values[pivot + 1..].reverse();
    true
}

fn random_case(location_count: usize, seed: u64, force_infeasible: bool) -> GeneratedCase {
    assert!(location_count >= 2);
    let mut rng = StdRng::seed_from_u64(seed ^ ((location_count as u64) << 32));
    let start_minutes: u16 = rng.gen_range(7 * 60..=10 * 60);
    let mut locations = Vec::with_capacity(location_count);
    for index in 0..location_count {
        let endpoint = index == 0 || index + 1 == location_count;
        let open: u16 = if endpoint {
            0
        } else {
            rng.gen_range(7 * 6..=13 * 6) * 10
        };
        let stay = if endpoint {
            0
        } else {
            [0_u32, 10, 20, 30][rng.gen_range(0..4)]
        };
        let close: u16 = if endpoint {
            23 * 60 + 50
        } else {
            rng.gen_range(((open + 20).min(23 * 60 + 50)) / 10..=143) * 10
        };
        locations.push(serde_json::json!({
            "id": format!("location-{index}"),
            "place_id": format!("place-{index}"),
            "open_time": hhmm(open),
            "close_time": hhmm(close),
            "stay_minutes": stay,
        }));
    }
    if force_infeasible {
        let blocked = if location_count == 2 {
            1
        } else {
            1 + seed as usize % (location_count - 2)
        };
        locations[blocked]["open_time"] = serde_json::json!("00:00");
        locations[blocked]["close_time"] = serde_json::json!("00:00");
        locations[blocked]["stay_minutes"] = serde_json::json!(10);
    }
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": format!("algorithm-validation-{location_count}-{seed}"),
        "locations": locations,
        "start_time": hhmm(start_minutes),
    }))
    .unwrap();
    let problem = request.try_into().unwrap();
    let matrix = TravelTimeMatrix::new(
        (0..location_count)
            .map(|from| {
                (0..location_count)
                    .map(|to| if from == to { 0 } else { rng.gen_range(1..=75) })
                    .collect()
            })
            .collect(),
    )
    .unwrap();
    GeneratedCase { problem, matrix }
}

fn boundary_case(travel: u32, stay: u32, open: u16, close: u16) -> GeneratedCase {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": format!("slot-boundary-{travel}-{stay}-{open}-{close}"),
        "locations": [
            {"id":"start","place_id":"start","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"end","place_id":"end","open_time":hhmm(open),"close_time":hhmm(close),"stay_minutes":stay}
        ],
        "start_time": "00:00"
    }))
    .unwrap();
    GeneratedCase {
        problem: request.try_into().unwrap(),
        matrix: TravelTimeMatrix::new(vec![vec![0, travel], vec![travel + 7, 0]]).unwrap(),
    }
}

fn all_day_case(location_count: usize, seed: u64) -> GeneratedCase {
    let mut rng = StdRng::seed_from_u64(seed ^ ((location_count as u64) << 32));
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": format!("initial-route-validation-{location_count}-{seed}"),
        "locations": (0..location_count).map(|index| serde_json::json!({
            "id": format!("location-{index}"),
            "place_id": format!("place-{index}"),
            "open_time": "00:00",
            "close_time": "23:50",
            "stay_minutes": 0
        })).collect::<Vec<_>>(),
        "start_time": "00:00"
    }))
    .unwrap();
    GeneratedCase {
        problem: request.try_into().unwrap(),
        matrix: TravelTimeMatrix::new(
            (0..location_count)
                .map(|from| {
                    (0..location_count)
                        .map(|to| if from == to { 0 } else { rng.gen_range(1..=75) })
                        .collect()
                })
                .collect(),
        )
        .unwrap(),
    }
}

fn hhmm(minutes: u16) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

fn solve_exact<F: FrontierPolicy>(
    case: &GeneratedCase,
    frontier: F,
) -> Result<troute::solver::ExactSolveResult, SolverError> {
    ExactBitDpSolver::new(DefaultObjectivePolicy, frontier).solve_detailed(SolverInput {
        matrix: &case.matrix,
        problem: &case.problem,
        cancellation: &CancellationToken::new(),
    })
}

#[derive(Clone, Copy)]
struct NoPruning;

impl FrontierPolicy for NoPruning {
    fn dominates(&self, _left: FrontierPoint, _right: FrontierPoint) -> bool {
        false
    }
}

#[test]
fn exact_matches_independent_brute_force_on_seeded_directed_cases() {
    // N=1 is covered by solver::tests::solves_one_location because the public
    // request contract intentionally starts at two locations.
    for location_count in 2..=10 {
        for (case_index, seed) in RANDOM_SEEDS.into_iter().enumerate() {
            let force_infeasible = case_index == 0;
            let case = random_case(location_count, seed, force_infeasible);
            let brute = brute_force(&case);
            let exact = solve_exact(&case, TimeCostFrontierPolicy);
            let context = format!("n={location_count}, seed={seed}");
            match (brute, exact) {
                (None, Err(SolverError::NoFeasibleRoute)) => {}
                (Some((_, expected)), Ok(actual)) => {
                    assert_eq!(actual.metrics, expected, "{context}");
                    let selected_start = troute::domain::TimeOfDay::from_minutes(
                        actual.metrics.start_time_slot * TIME_SLOT_MINUTES as u16,
                    )
                    .unwrap();
                    let schedule = calculate_schedule_from(
                        &case.problem,
                        &case.matrix,
                        &actual.solution,
                        selected_start,
                    )
                    .unwrap_or_else(|error| {
                        panic!("{context}: exact route is not schedulable: {error}")
                    });
                    assert_eq!(
                        schedule.total_travel_minutes, actual.metrics.travel_minutes,
                        "{context}"
                    );
                }
                (expected, actual) => panic!(
                    "{context}: feasibility differs: brute={}, exact={actual:?}",
                    expected.is_some()
                ),
            }
        }
    }
}

#[test]
fn pruning_on_and_off_produce_the_same_seeded_optimum() {
    for location_count in 2..=9 {
        for seed in [7_u64, 42, 1_337] {
            let case = random_case(location_count, seed, false);
            let pruned = solve_exact(&case, TimeCostFrontierPolicy);
            let exhaustive = solve_exact(&case, NoPruning);
            let context = format!("n={location_count}, seed={seed}");
            match (pruned, exhaustive) {
                (Ok(pruned), Ok(exhaustive)) => {
                    assert_eq!(pruned.metrics, exhaustive.metrics, "{context}");
                    assert!(
                        pruned.stats.frontier_states <= exhaustive.stats.frontier_states,
                        "{context}: pruning increased frontier states"
                    );
                    assert!(
                        pruned.stats.generated_states <= exhaustive.stats.generated_states,
                        "{context}: pruning increased generated states"
                    );
                }
                (Err(SolverError::NoFeasibleRoute), Err(SolverError::NoFeasibleRoute)) => {}
                (left, right) => {
                    panic!("{context}: pruning changed feasibility: {left:?} vs {right:?}")
                }
            }
        }
    }
}

#[test]
fn ten_minute_quantization_boundaries_are_conservative_and_schedulable() {
    for value in [1_u32, 9, 10, 11, 19, 20] {
        let travel = value;
        let stay = 0;
        let case = boundary_case(travel, stay, 0, 23 * 60 + 50);
        let result = solve_exact(&case, TimeCostFrontierPolicy).unwrap();
        let occupied_slots = travel.div_ceil(10);
        assert_eq!(
            result.metrics.start_time_slot,
            (143 - occupied_slots) as u16
        );
        assert_eq!(result.metrics.finish_time_slot, 143);
        let start = troute::domain::TimeOfDay::from_minutes(
            result.metrics.start_time_slot * TIME_SLOT_MINUTES as u16,
        )
        .unwrap();
        let schedule =
            calculate_schedule_from(&case.problem, &case.matrix, &result.solution, start)
                .unwrap_or_else(|error| panic!("travel={travel}, stay={stay}: {error}"));
        let actual_finish = schedule
            .stops
            .last()
            .unwrap()
            .departure_time
            .unwrap()
            .minutes();
        assert!(
            actual_finish <= result.metrics.finish_time_slot * TIME_SLOT_MINUTES as u16,
            "travel={travel}, stay={stay}"
        );
    }

    // API time windows are aligned, and slot 143 (23:50) is the final
    // representable finish slot.
    for (open, close, expected_start, expected_finish) in [
        (0, 0, 0, 0),
        (10, 10, 1, 1),
        (20, 20, 2, 2),
        (23 * 60 + 50, 23 * 60 + 50, 143, 143),
    ] {
        let case = boundary_case(0, 0, open, close);
        let result = solve_exact(&case, TimeCostFrontierPolicy).unwrap();
        assert_eq!(
            result.metrics.start_time_slot, expected_start,
            "open={open}, close={close}"
        );
        assert_eq!(
            result.metrics.finish_time_slot, expected_finish,
            "open={open}, close={close}"
        );
    }

    let impossible = boundary_case(0, 10, 23 * 60 + 50, 23 * 60 + 50);
    assert!(matches!(
        solve_exact(&impossible, TimeCostFrontierPolicy),
        Err(SolverError::NoFeasibleRoute)
    ));
}

#[test]
fn bit_dp_and_blossom_match_on_seeded_symmetric_instances() {
    for vertex_count in [2_usize, 4, 6, 8, 10, 12] {
        for seed in RANDOM_SEEDS {
            let mut rng = StdRng::seed_from_u64(seed ^ vertex_count as u64);
            let mut distances = vec![vec![0_u64; vertex_count]; vertex_count];
            let mut left = 0;
            while left < vertex_count {
                let mut right = left + 1;
                while right < vertex_count {
                    let distance = rng.gen_range(1..=10_000);
                    distances[left][right] = distance;
                    distances[right][left] = distance;
                    right += 1;
                }
                left += 1;
            }
            let vertices: Vec<_> = (0..vertex_count).collect();
            let bit_dp = BitDpPerfectMatching::default()
                .minimum_weight_perfect_matching(&vertices, &distances)
                .unwrap_or_else(|error| panic!("n={vertex_count}, seed={seed}: {error}"));
            let blossom = BlossomPerfectMatching
                .minimum_weight_perfect_matching(&vertices, &distances)
                .unwrap_or_else(|error| panic!("n={vertex_count}, seed={seed}: {error}"));
            let bit_cost: u64 = bit_dp.iter().map(|edge| edge.distance).sum();
            let blossom_cost: u64 = blossom.iter().map(|edge| edge.distance).sum();
            assert_eq!(bit_cost, blossom_cost, "n={vertex_count}, seed={seed}");
            for matching in [&bit_dp, &blossom] {
                let mut covered: Vec<_> = matching
                    .iter()
                    .flat_map(|edge| [edge.left, edge.right])
                    .collect();
                covered.sort_unstable();
                assert_eq!(covered, vertices, "n={vertex_count}, seed={seed}");
            }
        }
    }
}

#[test]
fn initial_route_generators_preserve_invariants_and_use_directed_costs() {
    for location_count in [3_usize, 5, 8, 10] {
        for seed in [42_u64, 1_337] {
            let case = all_day_case(location_count, seed);
            assert_initial_route("greedy", GreedyInitialRoute, &case, location_count, seed);
            assert_initial_route(
                "mst_double_tree",
                MstDoubleTreeInitialRoute::default(),
                &case,
                location_count,
                seed,
            );
            assert_initial_route(
                "christofides",
                ChristofidesInitialRoute::default(),
                &case,
                location_count,
                seed,
            );
        }
    }
}

fn assert_initial_route<G: InitialRouteGenerator>(
    strategy: &str,
    generator: G,
    case: &GeneratedCase,
    location_count: usize,
    seed: u64,
) {
    let cancellation = CancellationToken::new();
    let mut rng = StdRng::seed_from_u64(seed);
    let solution = generator
        .generate(
            SolverInput {
                matrix: &case.matrix,
                problem: &case.problem,
                cancellation: &cancellation,
            },
            &mut rng,
        )
        .unwrap_or_else(|error| panic!("{strategy}, n={location_count}, seed={seed}: {error}"));
    let context = format!("{strategy}, n={location_count}, seed={seed}");
    assert_eq!(solution.visit_order.len(), location_count, "{context}");
    assert_eq!(solution.visit_order.first(), Some(&0), "{context}");
    assert_eq!(
        solution.visit_order.last(),
        Some(&(location_count - 1)),
        "{context}"
    );
    assert_eq!(
        solution
            .visit_order
            .iter()
            .copied()
            .collect::<BTreeSet<_>>(),
        (0..location_count).collect(),
        "{context}"
    );
    let evaluated = evaluate_solution(
        &DefaultObjectivePolicy,
        SolverInput {
            matrix: &case.matrix,
            problem: &case.problem,
            cancellation: &cancellation,
        },
        &solution,
    )
    .unwrap_or_else(|error| panic!("{context}: {error}"));
    let directed_sum: u32 = solution
        .visit_order
        .windows(2)
        .map(|edge| case.matrix.travel_minutes(edge[0], edge[1]).unwrap())
        .sum();
    assert_eq!(evaluated.travel_minutes, directed_sum, "{context}");
}
