mod distance;
mod euler;
mod tree;

use rand::RngCore;

use super::{validate_generator_input, InitialRouteGenerator};
use crate::solver::{SolverError, SolverInput, SolverSolution};

pub use distance::*;
pub use euler::*;
pub(crate) use tree::minimum_spanning_tree;
pub use tree::MstEdge;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MstDoubleTreeResult {
    pub symmetric_distance_strategy: String,
    pub mst_edges: Vec<MstEdge>,
    pub euler_tour: Vec<usize>,
    pub solution: SolverSolution,
}

/// Deterministic MST double-tree route generator. The symmetric strategy is
/// used only for the MST; route evaluation always remains a caller concern and
/// uses the original directed matrix.
#[derive(Debug, Clone)]
pub struct MstDoubleTreeInitialRoute<S = AverageSymmetricDistance> {
    symmetric_distance: S,
}

impl Default for MstDoubleTreeInitialRoute {
    fn default() -> Self {
        Self {
            symmetric_distance: AverageSymmetricDistance,
        }
    }
}

impl<S> MstDoubleTreeInitialRoute<S> {
    pub fn new(symmetric_distance: S) -> Self {
        Self { symmetric_distance }
    }
}

impl<S: SymmetricDistanceStrategy> MstDoubleTreeInitialRoute<S> {
    pub fn generate_detailed(
        &self,
        input: SolverInput<'_>,
    ) -> Result<MstDoubleTreeResult, SolverError> {
        validate_generator_input(&input)?;
        let location_count = input.problem.locations().len();
        let edges = minimum_spanning_tree(location_count, |left, right| {
            self.symmetric_distance
                .symmetric_distance(input.matrix, left, right)
        })?;
        let euler_tour = doubled_tree_euler_tour(location_count, &edges, 0)?;
        let mut visit_order = shortcut_euler_tour(location_count, &euler_tour)?;

        // troute models a Hamiltonian path with distinct fixed endpoints, not
        // a TSP cycle. Preserve the endpoint contract after shortcutting.
        if location_count > 1 {
            let end = input.problem.end_location_index();
            let position = visit_order
                .iter()
                .position(|&location| location == end)
                .ok_or(SolverError::InvalidVisitOrder)?;
            visit_order.remove(position);
            visit_order.push(end);
        }

        Ok(MstDoubleTreeResult {
            symmetric_distance_strategy: self.symmetric_distance.name().to_owned(),
            mst_edges: edges,
            euler_tour,
            solution: SolverSolution { visit_order },
        })
    }
}

impl<S: SymmetricDistanceStrategy> InitialRouteGenerator for MstDoubleTreeInitialRoute<S> {
    fn generate(
        &self,
        input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        self.generate_detailed(input).map(|result| result.solution)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::{
        api::OptimizeRouteRequest,
        cancellation::CancellationToken,
        domain::OptimizationProblem,
        matrix::TravelTimeMatrix,
        solver::{evaluate_solution, DefaultObjectivePolicy, ExactBitDpSolver},
    };

    use super::*;

    fn problem(location_count: usize) -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "mst-double-tree-test",
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
            problem,
            matrix,
            cancellation,
        }
    }

    #[test]
    fn symmetric_distance_strategies_use_both_directed_values() {
        let matrix = TravelTimeMatrix::new(vec![vec![0, 10], vec![30, 0]]).unwrap();
        assert_eq!(
            AverageSymmetricDistance
                .symmetric_distance(&matrix, 0, 1)
                .unwrap(),
            20
        );
        assert_eq!(
            MinSymmetricDistance
                .symmetric_distance(&matrix, 0, 1)
                .unwrap(),
            10
        );
        assert_eq!(
            MaxSymmetricDistance
                .symmetric_distance(&matrix, 0, 1)
                .unwrap(),
            30
        );
    }

    #[test]
    fn prim_builds_the_expected_mst() {
        let weights = [[0, 1, 4, 8], [1, 0, 2, 5], [4, 2, 0, 3], [8, 5, 3, 0]];
        let edges = minimum_spanning_tree(4, |left, right| Ok(weights[left][right])).unwrap();
        assert_eq!(
            edges,
            vec![
                MstEdge {
                    from: 0,
                    to: 1,
                    distance: 1
                },
                MstEdge {
                    from: 1,
                    to: 2,
                    distance: 2
                },
                MstEdge {
                    from: 2,
                    to: 3,
                    distance: 3
                }
            ]
        );
    }

    #[test]
    fn doubled_edges_form_an_euler_walk_and_shortcut_once() {
        let edges = vec![
            MstEdge {
                from: 0,
                to: 1,
                distance: 1,
            },
            MstEdge {
                from: 0,
                to: 2,
                distance: 2,
            },
            MstEdge {
                from: 2,
                to: 3,
                distance: 1,
            },
        ];
        let euler = doubled_tree_euler_tour(4, &edges, 0).unwrap();
        assert_eq!(euler.len(), 2 * edges.len() + 1);
        assert_eq!(euler.first(), Some(&0));
        assert_eq!(euler.last(), Some(&0));
        assert_eq!(shortcut_euler_tour(4, &euler).unwrap(), vec![0, 1, 2, 3]);
    }

    #[test]
    fn generated_route_is_deterministic_and_visits_every_location_once() {
        let problem = problem(5);
        let matrix = TravelTimeMatrix::new(vec![
            vec![0, 2, 4, 6, 8],
            vec![3, 0, 2, 4, 6],
            vec![5, 3, 0, 2, 4],
            vec![7, 5, 3, 0, 2],
            vec![9, 7, 5, 3, 0],
        ])
        .unwrap();
        let cancellation = CancellationToken::new();
        let generator = MstDoubleTreeInitialRoute::default();
        let first = generator
            .generate_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();
        let second = generator
            .generate_detailed(input(&problem, &matrix, &cancellation))
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(first.symmetric_distance_strategy, "average_bidirectional");
        assert_eq!(first.mst_edges.len(), 4);
        assert_eq!(
            first
                .mst_edges
                .iter()
                .map(|edge| edge.distance)
                .sum::<u64>(),
            8
        );
        assert_eq!(first.euler_tour.len(), 2 * first.mst_edges.len() + 1);
        assert_eq!(first.solution.visit_order.first(), Some(&0));
        assert_eq!(first.solution.visit_order.last(), Some(&4));
        let unique: BTreeSet<_> = first.solution.visit_order.iter().copied().collect();
        assert_eq!(unique, (0..5).collect());
    }

    #[test]
    fn final_evaluation_uses_original_directed_matrix() {
        let problem = problem(4);
        let matrix = TravelTimeMatrix::new(vec![
            vec![0, 1, 50, 50],
            vec![99, 0, 2, 50],
            vec![50, 98, 0, 3],
            vec![50, 50, 97, 0],
        ])
        .unwrap();
        let cancellation = CancellationToken::new();
        let generated = MstDoubleTreeInitialRoute::default()
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
    fn metric_small_case_is_within_twice_the_exact_optimum() {
        let problem = problem(5);
        let matrix = TravelTimeMatrix::new(
            (0_u32..5)
                .map(|left| (0_u32..5).map(|right| left.abs_diff(right) * 10).collect())
                .collect(),
        )
        .unwrap();
        let cancellation = CancellationToken::new();
        let generated = MstDoubleTreeInitialRoute::default()
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

        assert!(generated_metrics.travel_minutes <= optimum.metrics.travel_minutes * 2);
    }
}
