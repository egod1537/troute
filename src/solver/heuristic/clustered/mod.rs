mod config;
mod improvement;
mod merge;
mod solver;
mod strategy;

pub use config::*;
pub use improvement::*;
use merge::*;
pub use solver::*;
pub use strategy::*;

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;
    use std::sync::{
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
        Arc,
    };

    use crate::{
        api::OptimizeRouteRequest,
        cancellation::CancellationToken,
        domain::{OptimizationProblem, TimeOfDay},
        matrix::TravelTimeMatrix,
        schedule::calculate_schedule_from,
        solver::{
            evaluate_solution, DefaultObjectivePolicy, ExactBitDpSolver, ObjectivePolicy,
            RouteSolver, SolverError, SolverInput, SolverSolution, TIME_SLOT_MINUTES,
        },
    };

    use super::*;

    fn problem(location_count: usize) -> OptimizationProblem {
        let locations: Vec<_> = (0..location_count)
            .map(|index| {
                serde_json::json!({
                    "id": index.to_string(),
                    "place_id": format!("place-{index}"),
                    "open_time": if index == 0 || index + 1 == location_count { "00:00" } else { "09:00" },
                    "close_time": "23:50",
                    "stay_minutes": if index == 0 || index + 1 == location_count { 0 } else { 10 }
                })
            })
            .collect();
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "clustered-solver-test",
            "locations": locations,
            "start_time": "09:00"
        }))
        .unwrap();
        request.try_into().unwrap()
    }

    fn directed_linear_matrix(size: usize) -> TravelTimeMatrix {
        let rows = (0..size)
            .map(|from| {
                (0..size)
                    .map(|to| {
                        if from == to {
                            0
                        } else if to == from + 1 {
                            10
                        } else if to > from {
                            30 + (to - from) as u32
                        } else {
                            200 + (from - to) as u32
                        }
                    })
                    .collect()
            })
            .collect();
        TravelTimeMatrix::new(rows).unwrap()
    }

    fn solve_clustered(
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
        max_cluster_size: usize,
    ) -> ClusteredSolveResult {
        ClusteredSolver::with_config(ClusteredSolverConfig::new(max_cluster_size).unwrap())
            .solve_detailed(SolverInput {
                matrix,
                problem,
                cancellation: &CancellationToken::new(),
            })
            .unwrap()
    }

    #[test]
    fn mst_cluster_order_is_deterministic_and_covers_each_cluster() {
        let problem = problem(7);
        let matrix = directed_linear_matrix(7);
        let clusters = vec![
            Cluster::new(vec![1, 2]),
            Cluster::new(vec![3, 4]),
            Cluster::new(vec![5]),
        ];
        let strategy = MstClusterOrderStrategy::default();
        let first = strategy.order(&clusters, &problem, &matrix).unwrap();
        let second = strategy.order(&clusters, &problem, &matrix).unwrap();

        assert_eq!(first, second);
        let mut covered = first;
        covered.sort_unstable();
        assert_eq!(covered, vec![0, 1, 2]);
    }

    #[test]
    fn large_route_contains_every_location_once_and_respects_cluster_limit() {
        let problem = problem(18);
        let matrix = directed_linear_matrix(18);
        let result = solve_clustered(&problem, &matrix, 5);

        assert_eq!(result.solution.visit_order.len(), 18);
        let mut locations = result.solution.visit_order.clone();
        locations.sort_unstable();
        assert_eq!(locations, (0..18).collect::<Vec<_>>());
        assert!(result.stats.cluster_sizes.iter().all(|&size| size <= 5));
        assert_eq!(result.stats.cluster_count, 4);
        assert_eq!(result.stats.cluster_strategy, "directed_nearest_neighbor");
        assert_eq!(result.stats.cluster_order_strategy, "greedy_bridge");
        assert_eq!(result.stats.cluster_order.len(), 4);
        assert_eq!(result.stats.cluster_details.len(), 4);
        assert!(result
            .stats
            .cluster_details
            .iter()
            .zip(&result.stats.cluster_order)
            .all(|(detail, order)| detail.cluster_index == *order));
        assert!(result.stats.cluster_details.iter().all(|detail| {
            detail.entry == detail.route.first().copied()
                && detail.exit == detail.route.last().copied()
                && detail.members.len() == detail.route.len()
                && detail.exact_generated_states > 0
                && detail.exact_frontier_states > 0
        }));
        assert!(result.stats.exact_generated_states > 0);
        assert!(result.stats.exact_frontier_states > 0);
        assert_eq!(result.stats.improvement_strategy, "boundary_swap");
        assert!(result.stats.improvement_operations.swap);
        assert!(!result.stats.improvement_operations.relocate);
        assert!(!result.stats.improvement_operations.two_opt);
    }

    #[test]
    fn merged_route_is_feasible_with_directed_times_and_time_windows() {
        let problem = problem(12);
        let matrix = directed_linear_matrix(12);
        let result = solve_clustered(&problem, &matrix, 4);

        assert_eq!(result.solution.visit_order, (0..12).collect::<Vec<_>>());
        let evaluated = evaluate_solution(
            &DefaultObjectivePolicy,
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            },
            &result.solution,
        )
        .unwrap();
        assert_eq!(evaluated, result.metrics);
    }

    #[test]
    fn restrictive_time_windows_are_preserved_across_cluster_merge() {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "clustered-time-window-test",
            "locations": [
                {"id":"0","place_id":"p0","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"1","place_id":"p1","open_time":"09:00","close_time":"10:00","stay_minutes":0},
                {"id":"2","place_id":"p2","open_time":"09:00","close_time":"11:00","stay_minutes":0},
                {"id":"3","place_id":"p3","open_time":"09:00","close_time":"12:00","stay_minutes":0},
                {"id":"4","place_id":"p4","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ],
            "start_time": "09:00"
        }))
        .unwrap();
        let problem: OptimizationProblem = request.try_into().unwrap();
        let matrix = TravelTimeMatrix::new(
            (0..5)
                .map(|from| (0..5).map(|to| if from == to { 0 } else { 10 }).collect())
                .collect(),
        )
        .unwrap();
        let result = solve_clustered(&problem, &matrix, 2);

        assert_eq!(result.metrics.start_time_slot, 59); // 09:50
        let start_time =
            TimeOfDay::from_minutes(result.metrics.start_time_slot * TIME_SLOT_MINUTES as u16)
                .unwrap();
        let plan =
            calculate_schedule_from(&problem, &matrix, &result.solution, start_time).unwrap();
        let constrained = plan
            .stops
            .iter()
            .find(|stop| stop.location_index == 1)
            .unwrap();
        assert!(constrained.departure_time.unwrap().minutes() <= 10 * 60);
    }

    #[test]
    fn local_improvement_never_worsens_the_shared_objective() {
        let problem = problem(14);
        let matrix = directed_linear_matrix(14);
        let result = solve_clustered(&problem, &matrix, 4);

        assert_ne!(
            DefaultObjectivePolicy.compare(&result.metrics, &result.metrics_before_improvement),
            Ordering::Greater
        );
    }

    #[test]
    fn small_problem_can_be_compared_with_exact_solution() {
        let problem = problem(8);
        let matrix = directed_linear_matrix(8);
        let cancellation = CancellationToken::new();
        let exact = ExactBitDpSolver::default()
            .solve_detailed(SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &cancellation,
            })
            .unwrap();
        let clustered = solve_clustered(&problem, &matrix, 3);

        assert_eq!(clustered.solution, exact.solution);
        assert_eq!(clustered.metrics, exact.metrics);
    }

    #[derive(Clone)]
    struct CountingSolver {
        calls: Arc<AtomicUsize>,
    }

    impl RouteSolver for CountingSolver {
        fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
            self.calls.fetch_add(1, AtomicOrdering::Relaxed);
            Ok(SolverSolution {
                visit_order: (0..input.problem.locations().len()).collect(),
            })
        }
    }

    #[test]
    fn automatic_selection_uses_exact_at_limit_and_clustered_above_it() {
        let exact_calls = Arc::new(AtomicUsize::new(0));
        let clustered_calls = Arc::new(AtomicUsize::new(0));
        let solver = AutomaticRouteSolver::new(
            CountingSolver {
                calls: Arc::clone(&exact_calls),
            },
            CountingSolver {
                calls: Arc::clone(&clustered_calls),
            },
            ThresholdSolverSelectionPolicy::new(3).unwrap(),
        );

        for size in [3, 4] {
            let problem = problem(size);
            let matrix = directed_linear_matrix(size);
            solver
                .solve(SolverInput {
                    matrix: &matrix,
                    problem: &problem,
                    cancellation: &CancellationToken::new(),
                })
                .unwrap();
        }
        assert_eq!(exact_calls.load(AtomicOrdering::Relaxed), 1);
        assert_eq!(clustered_calls.load(AtomicOrdering::Relaxed), 1);
    }
}
