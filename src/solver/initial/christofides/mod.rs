pub mod euler;
pub mod matching;
pub mod odd_vertices;
mod solver;

pub use euler::*;
pub use matching::*;
pub use odd_vertices::*;
pub use solver::*;

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rand::{rngs::StdRng, SeedableRng};

    use crate::{
        api::OptimizeRouteRequest, cancellation::CancellationToken, domain::OptimizationProblem,
        matrix::TravelTimeMatrix,
    };

    use super::*;
    use crate::solver::{
        evaluate_solution, AverageSymmetricDistance, DefaultObjectivePolicy, ExactBitDpSolver,
        InitialRouteGenerator, MixedNeighborhoodStrategy, MstEdge, SimulatedAnnealingConfig,
        SimulatedAnnealingSolver, SolverError, SolverInput, SymmetricDistanceStrategy,
    };

    fn problem(location_count: usize) -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "christofides-test",
            "locations": (0..location_count).map(|index| serde_json::json!({
                "id": index.to_string(),
                "place_id": format!("p-{index}"),
                "open_time": "00:00",
                "close_time": "23:50",
                "stay_minutes": 0
            })).collect::<Vec<_>>(),
            "start_time": "00:00"
        }))
        .unwrap();
        request.try_into().unwrap()
    }

    fn input<'a>(
        problem: &'a OptimizationProblem,
        matrix: &'a TravelTimeMatrix,
        cancellation: &'a CancellationToken,
    ) -> SolverInput<'a> {
        SolverInput {
            matrix,
            problem,
            cancellation,
        }
    }

    fn line_metric(size: usize) -> TravelTimeMatrix {
        TravelTimeMatrix::new(
            (0..size)
                .map(|left| {
                    (0..size)
                        .map(|right| left.abs_diff(right) as u32 * 10)
                        .collect()
                })
                .collect(),
        )
        .unwrap()
    }

    fn matching_distances(size: usize) -> Vec<Vec<u64>> {
        (0..size)
            .map(|left| {
                (0..size)
                    .map(|right| {
                        if left == right {
                            0
                        } else {
                            left.abs_diff(right) as u64 * 7 + ((left + right) % 5) as u64
                        }
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn tree_has_an_even_number_of_odd_degree_vertices() {
        let edges = vec![
            MstEdge {
                from: 0,
                to: 1,
                distance: 1,
            },
            MstEdge {
                from: 0,
                to: 2,
                distance: 1,
            },
            MstEdge {
                from: 0,
                to: 3,
                distance: 1,
            },
            MstEdge {
                from: 3,
                to: 4,
                distance: 1,
            },
        ];
        let odd = odd_degree_vertices(5, &edges).unwrap();
        assert_eq!(odd.len() % 2, 0);
        assert_eq!(odd, vec![0, 1, 2, 4]);
    }

    #[test]
    fn bit_dp_finds_the_exact_minimum_matching_and_covers_each_vertex_once() {
        let vertices = vec![10, 11, 12, 13];
        let distances = vec![
            vec![0, 1, 2, 2],
            vec![1, 0, 100, 2],
            vec![2, 100, 0, 100],
            vec![2, 2, 100, 0],
        ];
        let matching = BitDpPerfectMatching::default()
            .minimum_weight_perfect_matching(&vertices, &distances)
            .unwrap();

        // Greedily taking 10-11 would cost 101 in total; exact DP finds 4.
        assert_eq!(matching.iter().map(|edge| edge.distance).sum::<u64>(), 4);
        let mut covered: Vec<_> = matching
            .iter()
            .flat_map(|edge| [edge.left, edge.right])
            .collect();
        covered.sort_unstable();
        assert_eq!(covered, vertices);
    }

    #[test]
    fn bit_dp_rejects_an_odd_set_above_its_limit() {
        let vertices: Vec<_> = (0..22).collect();
        let distances = vec![vec![1; 22]; 22];
        assert!(matches!(
            BitDpPerfectMatching::default().minimum_weight_perfect_matching(&vertices, &distances),
            Err(SolverError::UnsupportedMatchingVertexCount {
                maximum: MAX_BIT_DP_MATCHING_VERTICES,
                actual: 22
            })
        ));
    }

    #[test]
    fn blossom_and_bit_dp_have_the_same_small_instance_cost() {
        let vertices: Vec<_> = (0..12).collect();
        let distances = matching_distances(vertices.len());
        let bit_dp = BitDpPerfectMatching::default()
            .minimum_weight_perfect_matching(&vertices, &distances)
            .unwrap();
        let blossom = BlossomPerfectMatching
            .minimum_weight_perfect_matching(&vertices, &distances)
            .unwrap();

        assert_eq!(matching_cost(&bit_dp), matching_cost(&blossom));
    }

    #[test]
    fn blossom_handles_a_large_odd_set_completely_and_deterministically() {
        let vertices: Vec<_> = (0..64).collect();
        let distances = matching_distances(vertices.len());
        let solve = || {
            BlossomPerfectMatching
                .minimum_weight_perfect_matching(&vertices, &distances)
                .unwrap()
        };
        let first = solve();
        let second = solve();

        assert_eq!(first, second);
        assert_eq!(first.len(), vertices.len() / 2);
        let mut covered: Vec<_> = first
            .iter()
            .flat_map(|edge| [edge.left, edge.right])
            .collect();
        covered.sort_unstable();
        assert_eq!(covered, vertices);
    }

    #[test]
    fn auto_and_forced_matching_selection_obey_configuration() {
        let default = AutoPerfectMatching::default();
        assert_eq!(default.selected_strategy(20), MatchingStrategyChoice::BitDp);
        assert_eq!(
            default.selected_strategy(22),
            MatchingStrategyChoice::Blossom
        );

        let forced_blossom = AutoPerfectMatching::new(
            MatchingStrategyConfig::new(MatchingStrategyChoice::Blossom, 20).unwrap(),
        )
        .unwrap();
        assert_eq!(
            forced_blossom.selected_strategy(4),
            MatchingStrategyChoice::Blossom
        );
        let forced_bit_dp = AutoPerfectMatching::new(
            MatchingStrategyConfig::new(MatchingStrategyChoice::BitDp, 2).unwrap(),
        )
        .unwrap();
        assert_eq!(
            forced_bit_dp.selected_strategy(20),
            MatchingStrategyChoice::BitDp
        );
        assert_eq!(
            "auto".parse::<MatchingStrategyChoice>().unwrap(),
            MatchingStrategyChoice::Auto
        );
        assert_eq!(
            "bitdp".parse::<MatchingStrategyChoice>().unwrap(),
            MatchingStrategyChoice::BitDp
        );
        assert_eq!(
            "blossom".parse::<MatchingStrategyChoice>().unwrap(),
            MatchingStrategyChoice::Blossom
        );
    }

    #[test]
    fn christofides_generates_a_complete_route_with_forced_blossom() {
        let problem = problem(12);
        let matrix = line_metric(12);
        let cancellation = CancellationToken::new();
        let result =
            ChristofidesInitialRoute::new(AverageSymmetricDistance, BlossomPerfectMatching)
                .generate_detailed(input(&problem, &matrix, &cancellation))
                .unwrap();

        let visited: BTreeSet<_> = result.solution.visit_order.iter().copied().collect();
        assert_eq!(visited, (0..12).collect());
        assert_eq!(result.solution.visit_order.first(), Some(&0));
        assert_eq!(result.solution.visit_order.last(), Some(&11));
        assert_eq!(result.matching_strategy, "blossom");
    }

    #[test]
    fn mst_plus_matching_is_eulerian_and_shortcuts_every_vertex_once() {
        let problem = problem(6);
        let matrix = line_metric(6);
        let cancellation = CancellationToken::new();
        let result = ChristofidesInitialRoute::default()
            .generate_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();

        let odd_set: BTreeSet<_> = result.odd_vertices.iter().copied().collect();
        let matched: BTreeSet<_> = result
            .matching_edges
            .iter()
            .flat_map(|edge| [edge.left, edge.right])
            .collect();
        assert_eq!(odd_set, matched);
        assert_eq!(result.symmetric_distance_strategy, "average_bidirectional");
        assert_eq!(result.matching_strategy, "bit_dp");
        assert!(odd_degree_vertices(6, &result.eulerian_edges)
            .unwrap()
            .is_empty());
        assert_eq!(result.euler_tour.len(), result.eulerian_edges.len() + 1);
        let visited: BTreeSet<_> = result.solution.visit_order.iter().copied().collect();
        assert_eq!(visited, (0..6).collect());
        assert_eq!(result.solution.visit_order.len(), 6);
    }

    #[test]
    fn small_metric_case_is_within_three_halves_of_exact_optimum() {
        let problem = problem(7);
        let matrix = line_metric(7);
        let cancellation = CancellationToken::new();
        let generated = ChristofidesInitialRoute::default()
            .generate_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();
        let generated_metrics = evaluate_solution(
            &DefaultObjectivePolicy,
            input(&problem, &matrix, &cancellation),
            &generated.solution,
        )
        .unwrap();
        let optimum = ExactBitDpSolver::default()
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();

        assert!(generated_metrics.travel_minutes * 2 <= optimum.metrics.travel_minutes * 3);
    }

    #[test]
    fn final_evaluation_uses_the_original_directed_matrix() {
        let problem = problem(4);
        let matrix = TravelTimeMatrix::new(vec![
            vec![0, 1, 50, 50],
            vec![99, 0, 2, 50],
            vec![50, 98, 0, 3],
            vec![50, 50, 97, 0],
        ])
        .unwrap();
        let cancellation = CancellationToken::new();
        let generated = ChristofidesInitialRoute::default()
            .generate_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();
        let evaluated = evaluate_solution(
            &DefaultObjectivePolicy,
            input(&problem, &matrix, &cancellation),
            &generated.solution,
        )
        .unwrap();
        let directed_sum: u32 = generated
            .solution
            .visit_order
            .windows(2)
            .map(|edge| matrix.travel_minutes(edge[0], edge[1]).unwrap())
            .sum();
        let symmetric_sum: u64 = generated
            .solution
            .visit_order
            .windows(2)
            .map(|edge| {
                AverageSymmetricDistance
                    .symmetric_distance(&matrix, edge[0], edge[1])
                    .unwrap()
            })
            .sum();

        assert_eq!(evaluated.travel_minutes, directed_sum);
        assert_ne!(u64::from(evaluated.travel_minutes), symmetric_sum);
    }

    #[test]
    fn generator_can_seed_simulated_annealing() {
        let problem = problem(7);
        let matrix = line_metric(7);
        let cancellation = CancellationToken::new();
        let solver = SimulatedAnnealingSolver::new(
            SimulatedAnnealingConfig {
                iteration_limit: Some(50),
                seed: Some(42),
                ..SimulatedAnnealingConfig::default()
            },
            ChristofidesInitialRoute::default(),
            MixedNeighborhoodStrategy,
            DefaultObjectivePolicy,
        )
        .unwrap();
        let result = solver
            .solve_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();

        assert_eq!(result.solution.visit_order.first(), Some(&0));
        assert_eq!(result.solution.visit_order.last(), Some(&6));
    }

    #[test]
    fn generation_is_deterministic() {
        let problem = problem(8);
        let matrix = line_metric(8);
        let cancellation = CancellationToken::new();
        let generator = ChristofidesInitialRoute::default();
        let first = generator
            .generate(
                input(&problem, &matrix, &cancellation),
                &mut StdRng::seed_from_u64(1),
            )
            .unwrap();
        let second = generator
            .generate(
                input(&problem, &matrix, &cancellation),
                &mut StdRng::seed_from_u64(999),
            )
            .unwrap();
        assert_eq!(first, second);
    }

    fn matching_cost(matching: &[PerfectMatchingEdge]) -> u64 {
        matching.iter().map(|edge| edge.distance).sum()
    }
}
