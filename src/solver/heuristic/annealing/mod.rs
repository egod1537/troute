mod config;
mod neighbor;
mod penalty;
mod solver;

pub use config::*;
pub use neighbor::*;
pub use penalty::*;
pub use solver::*;

#[cfg(test)]
mod tests {
    use std::{sync::Mutex, thread, time::Duration};

    use rand::{rngs::StdRng, RngCore, SeedableRng};

    use crate::{
        api::OptimizeRouteRequest,
        cancellation::CancellationToken,
        domain::OptimizationProblem,
        matrix::TravelTimeMatrix,
        solver::{
            evaluate_solution, DefaultObjectivePolicy, MstDoubleTreeInitialRoute, SolverError,
            SolverInput, SolverSolution,
        },
    };

    use super::*;

    fn problem(location_count: usize) -> OptimizationProblem {
        let locations: Vec<_> = (0..location_count)
            .map(|index| {
                serde_json::json!({
                    "id": index.to_string(),
                    "place_id": format!("place-{index}"),
                    "open_time": "09:00",
                    "close_time": "23:50",
                    "stay_minutes": if index == 0 || index + 1 == location_count { 0 } else { 10 }
                })
            })
            .collect();
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "simulated-annealing-test",
            "locations": locations,
            "start_time": "09:00"
        }))
        .unwrap();
        request.try_into().unwrap()
    }

    fn directed_matrix(size: usize) -> TravelTimeMatrix {
        TravelTimeMatrix::new(
            (0..size)
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
                                100 + (from - to) as u32
                            }
                        })
                        .collect()
                })
                .collect(),
        )
        .unwrap()
    }

    fn deterministic_config(iterations: u64) -> SimulatedAnnealingConfig {
        SimulatedAnnealingConfig {
            initial_temperature: 1_000.0,
            cooling_rate: 0.99,
            minimum_temperature: 0.0001,
            iteration_limit: Some(iterations),
            time_limit_ms: None,
            seed: Some(42),
            penalties: InfeasiblePenaltyWeights::default(),
        }
    }

    #[test]
    fn neighborhood_operations_preserve_endpoints_and_permutation() {
        let original = vec![0, 1, 2, 3, 4];

        let mut swapped = original.clone();
        assert!(apply_neighborhood_move(
            &mut swapped,
            NeighborhoodMove::Swap { left: 1, right: 3 }
        ));
        assert_eq!(swapped, vec![0, 3, 2, 1, 4]);

        let mut relocated = original.clone();
        assert!(apply_neighborhood_move(
            &mut relocated,
            NeighborhoodMove::Relocate { from: 1, to: 3 }
        ));
        assert_eq!(relocated, vec![0, 2, 3, 1, 4]);

        let mut reversed = original.clone();
        assert!(apply_neighborhood_move(
            &mut reversed,
            NeighborhoodMove::TwoOpt { start: 1, end: 3 }
        ));
        assert_eq!(reversed, vec![0, 3, 2, 1, 4]);
    }

    #[test]
    fn fixed_seed_is_deterministic() {
        let problem = problem(7);
        let matrix = directed_matrix(7);
        let solve = || {
            SimulatedAnnealingSolver::with_config(deterministic_config(250))
                .unwrap()
                .solve_detailed(SolverInput {
                    matrix: &matrix,
                    problem: &problem,
                    cancellation: &CancellationToken::new(),
                })
                .unwrap()
        };
        let first = solve();
        let second = solve();

        assert_eq!(first.solution, second.solution);
        assert_eq!(first.initial_solution, second.initial_solution);
        assert_eq!(first.initial_metrics, second.initial_metrics);
        assert_eq!(first.metrics, second.metrics);
        assert_eq!(first.stats.iterations, second.stats.iterations);
        assert_eq!(first.stats.accepted_moves, second.stats.accepted_moves);
        assert_eq!(first.stats.improved_moves, second.stats.improved_moves);
        assert_eq!(
            first.stats.swap_moves + first.stats.relocate_moves + first.stats.two_opt_moves,
            first.stats.iterations
        );
        assert_eq!(first.stats.initial_temperature, 1_000.0);
        assert_eq!(first.stats.cooling_rate, 0.99);
        assert!(first.stats.final_temperature < first.stats.initial_temperature);
        assert_eq!(
            first.stats.accepted_infeasible_moves,
            second.stats.accepted_infeasible_moves
        );
    }

    #[test]
    fn mst_double_tree_generator_can_seed_annealing() {
        let problem = problem(7);
        let matrix = directed_matrix(7);
        let cancellation = CancellationToken::new();
        let result = SimulatedAnnealingSolver::new(
            deterministic_config(50),
            MstDoubleTreeInitialRoute::default(),
            MixedNeighborhoodStrategy,
            DefaultObjectivePolicy,
        )
        .unwrap()
        .solve_detailed(SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &cancellation,
        })
        .unwrap();

        assert_eq!(result.solution.visit_order.len(), 7);
        assert!(evaluate_solution(
            &DefaultObjectivePolicy,
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &cancellation,
            },
            &result.solution,
        )
        .is_ok());
    }

    #[test]
    fn worse_move_acceptance_is_temperature_and_probability_based() {
        assert!(simulated_annealing_accept(0.0, 1.0, 0.99));
        assert!(simulated_annealing_accept(-1.0, 0.0, 0.99));
        assert!(simulated_annealing_accept(10.0, 100.0, 0.5));
        assert!(!simulated_annealing_accept(10.0, 100.0, 0.99));
        assert!(!simulated_annealing_accept(10.0, 0.0, 0.0));
    }

    fn constrained_problem() -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "sa-infeasible-test",
            "locations": [
                {"id":"0","place_id":"p0","open_time":"00:00","close_time":"23:50","stay_minutes":0},
                {"id":"1","place_id":"p1","open_time":"09:00","close_time":"09:30","stay_minutes":0},
                {"id":"2","place_id":"p2","open_time":"09:00","close_time":"23:50","stay_minutes":0},
                {"id":"3","place_id":"p3","open_time":"00:00","close_time":"23:50","stay_minutes":0}
            ],
            "start_time": "09:00"
        }))
        .unwrap();
        request.try_into().unwrap()
    }

    fn constrained_matrix() -> TravelTimeMatrix {
        TravelTimeMatrix::new(vec![
            vec![0, 10, 10, 10],
            vec![10, 0, 10, 10],
            vec![10, 30, 0, 10],
            vec![10, 10, 10, 0],
        ])
        .unwrap()
    }

    #[test]
    fn penalty_evaluator_keeps_infeasible_routes_in_the_search_space() {
        let problem = constrained_problem();
        let matrix = constrained_matrix();
        let evaluation = evaluate_solution_with_penalties(
            &DefaultObjectivePolicy,
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            },
            &SolverSolution {
                visit_order: vec![0, 2, 1, 3],
            },
            InfeasiblePenaltyWeights::default(),
        )
        .unwrap();

        assert!(!evaluation.feasible);
        assert!(evaluation.late_minutes > 0);
        assert!(evaluation.penalty > 0.0);
        assert!(evaluation.energy.is_finite());
    }

    #[derive(Clone)]
    struct FixedInitial(Vec<usize>);

    impl InitialRouteStrategy for FixedInitial {
        fn initial_solution(
            &self,
            _input: SolverInput<'_>,
            _rng: &mut dyn RngCore,
        ) -> Result<SolverSolution, SolverError> {
            Ok(SolverSolution {
                visit_order: self.0.clone(),
            })
        }
    }

    #[derive(Default)]
    struct AlwaysSwap;

    impl NeighborhoodStrategy for AlwaysSwap {
        fn neighbor(
            &self,
            route: &[usize],
            _rng: &mut dyn RngCore,
        ) -> Option<(Vec<usize>, NeighborhoodMove)> {
            let movement = NeighborhoodMove::Swap { left: 1, right: 2 };
            let mut neighbor = route.to_vec();
            apply_neighborhood_move(&mut neighbor, movement).then_some((neighbor, movement))
        }
    }

    #[test]
    fn infeasible_states_can_be_accepted_but_best_feasible_is_returned() {
        let problem = constrained_problem();
        let matrix = constrained_matrix();
        let config = SimulatedAnnealingConfig {
            initial_temperature: 1.0e12,
            cooling_rate: 0.99,
            minimum_temperature: 1.0,
            iteration_limit: Some(20),
            time_limit_ms: None,
            seed: Some(7),
            penalties: InfeasiblePenaltyWeights {
                late_minute_weight: 1.0,
                ..InfeasiblePenaltyWeights::default()
            },
        };
        let solver = SimulatedAnnealingSolver::new(
            config,
            FixedInitial(vec![0, 1, 2, 3]),
            AlwaysSwap,
            DefaultObjectivePolicy,
        )
        .unwrap();
        let result = solver
            .solve_detailed(SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            })
            .unwrap();

        assert!(result.stats.infeasible_candidates > 0);
        assert!(result.stats.accepted_infeasible_moves > 0);
        assert_eq!(result.solution.visit_order, vec![0, 1, 2, 3]);
        assert!(evaluate_solution(
            &DefaultObjectivePolicy,
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            },
            &result.solution,
        )
        .is_ok());
    }

    #[derive(Default)]
    struct SlowSwap {
        calls: Mutex<u64>,
    }

    impl NeighborhoodStrategy for SlowSwap {
        fn neighbor(
            &self,
            route: &[usize],
            _rng: &mut dyn RngCore,
        ) -> Option<(Vec<usize>, NeighborhoodMove)> {
            thread::sleep(Duration::from_millis(2));
            *self.calls.lock().unwrap() += 1;
            AlwaysSwap.neighbor(route, &mut StdRng::seed_from_u64(1))
        }
    }

    #[test]
    fn iteration_and_time_limits_are_honored() {
        let problem = problem(5);
        let matrix = directed_matrix(5);
        let iteration_result = SimulatedAnnealingSolver::new(
            deterministic_config(17),
            FixedInitial(vec![0, 1, 2, 3, 4]),
            AlwaysSwap,
            DefaultObjectivePolicy,
        )
        .unwrap()
        .solve_detailed(SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        })
        .unwrap();
        assert_eq!(iteration_result.stats.iterations, 17);

        let time_config = SimulatedAnnealingConfig {
            iteration_limit: Some(1_000_000),
            time_limit_ms: Some(5),
            ..deterministic_config(1_000_000)
        };
        let timed = SimulatedAnnealingSolver::new(
            time_config,
            FixedInitial(vec![0, 1, 2, 3, 4]),
            SlowSwap::default(),
            DefaultObjectivePolicy,
        )
        .unwrap()
        .solve_detailed(SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        })
        .unwrap();
        assert!(timed.stats.iterations < 1_000_000);
        assert!(timed.stats.elapsed < Duration::from_millis(200));
    }

    #[test]
    fn feasible_penalty_evaluation_reuses_shared_objective_metrics() {
        let problem = problem(5);
        let matrix = directed_matrix(5);
        let solution = SolverSolution {
            visit_order: vec![0, 1, 2, 3, 4],
        };
        let shared = evaluate_solution(
            &DefaultObjectivePolicy,
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            },
            &solution,
        )
        .unwrap();
        let penalized = evaluate_solution_with_penalties(
            &DefaultObjectivePolicy,
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            },
            &solution,
            InfeasiblePenaltyWeights::default(),
        )
        .unwrap();

        assert!(penalized.feasible);
        assert_eq!(penalized.feasible_metrics, Some(shared));
    }
}
