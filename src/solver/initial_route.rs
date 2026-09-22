use std::collections::BTreeSet;

use rand::{seq::SliceRandom, RngCore};

use crate::matrix::TravelTimeMatrix;

use super::{ClusteredSolver, RouteSolver, SolverError, SolverInput, SolverSolution};

/// Produces a complete visit order without requiring it to be feasible.
/// Consumers must evaluate the returned route against the original directed
/// matrix and the normal schedule constraints.
pub trait InitialRouteGenerator: Send + Sync {
    fn generate(
        &self,
        input: SolverInput<'_>,
        rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError>;
}

/// Converts the directed travel-time matrix into the undirected weights used
/// only while building an MST.
pub trait SymmetricDistanceStrategy: Send + Sync {
    fn symmetric_distance(
        &self,
        matrix: &TravelTimeMatrix,
        left: usize,
        right: usize,
    ) -> Result<u64, SolverError>;
}

fn directed_pair(
    matrix: &TravelTimeMatrix,
    left: usize,
    right: usize,
) -> Result<(u32, u32), SolverError> {
    let forward = matrix.travel_minutes(left, right).ok_or_else(|| {
        SolverError::InvalidConfiguration(format!(
            "symmetric distance index {left}->{right} is outside the matrix"
        ))
    })?;
    let reverse = matrix.travel_minutes(right, left).ok_or_else(|| {
        SolverError::InvalidConfiguration(format!(
            "symmetric distance index {right}->{left} is outside the matrix"
        ))
    })?;
    Ok((forward, reverse))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AverageSymmetricDistance;

impl SymmetricDistanceStrategy for AverageSymmetricDistance {
    fn symmetric_distance(
        &self,
        matrix: &TravelTimeMatrix,
        left: usize,
        right: usize,
    ) -> Result<u64, SolverError> {
        let (forward, reverse) = directed_pair(matrix, left, right)?;
        Ok((u64::from(forward) + u64::from(reverse)) / 2)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MinSymmetricDistance;

impl SymmetricDistanceStrategy for MinSymmetricDistance {
    fn symmetric_distance(
        &self,
        matrix: &TravelTimeMatrix,
        left: usize,
        right: usize,
    ) -> Result<u64, SolverError> {
        let (forward, reverse) = directed_pair(matrix, left, right)?;
        Ok(u64::from(forward.min(reverse)))
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MaxSymmetricDistance;

impl SymmetricDistanceStrategy for MaxSymmetricDistance {
    fn symmetric_distance(
        &self,
        matrix: &TravelTimeMatrix,
        left: usize,
        right: usize,
    ) -> Result<u64, SolverError> {
        let (forward, reverse) = directed_pair(matrix, left, right)?;
        Ok(u64::from(forward.max(reverse)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MstEdge {
    pub from: usize,
    pub to: usize,
    pub distance: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MstDoubleTreeResult {
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

/// Prim's algorithm with stable index tie-breaking.
pub(crate) fn minimum_spanning_tree<F>(
    node_count: usize,
    mut distance: F,
) -> Result<Vec<MstEdge>, SolverError>
where
    F: FnMut(usize, usize) -> Result<u64, SolverError>,
{
    if node_count == 0 {
        return Err(SolverError::InvalidConfiguration(
            "an MST requires at least one node".to_owned(),
        ));
    }
    let mut included = vec![false; node_count];
    let mut best_distance = vec![u64::MAX; node_count];
    let mut parent = vec![None; node_count];
    best_distance[0] = 0;
    let mut edges = Vec::with_capacity(node_count.saturating_sub(1));

    for _ in 0..node_count {
        let next = (0..node_count)
            .filter(|&node| !included[node])
            .min_by_key(|&node| (best_distance[node], node))
            .ok_or_else(|| SolverError::Failed("MST construction failed".to_owned()))?;
        if best_distance[next] == u64::MAX {
            return Err(SolverError::Failed("MST graph is not connected".to_owned()));
        }
        included[next] = true;
        if let Some(from) = parent[next] {
            edges.push(MstEdge {
                from,
                to: next,
                distance: best_distance[next],
            });
        }

        for candidate in 0..node_count {
            if included[candidate] || candidate == next {
                continue;
            }
            let candidate_distance = distance(next, candidate)?;
            let better_parent = match parent[candidate] {
                Some(old_parent) => next < old_parent,
                None => true,
            };
            let is_better = candidate_distance < best_distance[candidate]
                || (candidate_distance == best_distance[candidate] && better_parent);
            if is_better {
                best_distance[candidate] = candidate_distance;
                parent[candidate] = Some(next);
            }
        }
    }
    Ok(edges)
}

/// Returns the deterministic Euler walk of the multigraph obtained by
/// doubling every tree edge.
pub fn doubled_tree_euler_tour(
    node_count: usize,
    edges: &[MstEdge],
    root: usize,
) -> Result<Vec<usize>, SolverError> {
    if node_count == 0 || root >= node_count || edges.len() != node_count.saturating_sub(1) {
        return Err(SolverError::InvalidConfiguration(
            "Euler tour input must be a rooted spanning tree".to_owned(),
        ));
    }
    let mut adjacency = vec![Vec::<(usize, u64)>::new(); node_count];
    for edge in edges {
        if edge.from >= node_count || edge.to >= node_count || edge.from == edge.to {
            return Err(SolverError::InvalidConfiguration(
                "MST edge contains an invalid endpoint".to_owned(),
            ));
        }
        adjacency[edge.from].push((edge.to, edge.distance));
        adjacency[edge.to].push((edge.from, edge.distance));
    }
    for neighbors in &mut adjacency {
        neighbors.sort_by_key(|&(node, distance)| (distance, node));
    }

    let mut seen = vec![false; node_count];
    seen[root] = true;
    let mut tour = vec![root];
    let mut stack = vec![(root, 0_usize)];
    while let Some((node, next_neighbor)) = stack.last_mut() {
        if *next_neighbor < adjacency[*node].len() {
            let child = adjacency[*node][*next_neighbor].0;
            *next_neighbor += 1;
            if !seen[child] {
                seen[child] = true;
                tour.push(child);
                stack.push((child, 0));
            }
        } else {
            stack.pop();
            if let Some((parent, _)) = stack.last() {
                tour.push(*parent);
            }
        }
    }
    if seen.iter().any(|visited| !visited) {
        return Err(SolverError::InvalidConfiguration(
            "MST edges do not connect every node".to_owned(),
        ));
    }
    Ok(tour)
}

pub fn shortcut_euler_tour(
    node_count: usize,
    euler_tour: &[usize],
) -> Result<Vec<usize>, SolverError> {
    let mut seen = vec![false; node_count];
    let mut route = Vec::with_capacity(node_count);
    for &node in euler_tour {
        if node >= node_count {
            return Err(SolverError::InvalidVisitOrder);
        }
        if !seen[node] {
            seen[node] = true;
            route.push(node);
        }
    }
    if route.len() != node_count {
        return Err(SolverError::InvalidVisitOrder);
    }
    Ok(route)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GreedyInitialRoute;

impl InitialRouteGenerator for GreedyInitialRoute {
    fn generate(
        &self,
        input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        validate_generator_input(&input)?;
        let end = input.problem.end_location_index();
        if end == 0 {
            return Ok(SolverSolution {
                visit_order: vec![0],
            });
        }
        let mut remaining: BTreeSet<_> = (1..end).collect();
        let mut visit_order = vec![input.problem.start_location_index()];
        let mut current = input.problem.start_location_index();
        while !remaining.is_empty() {
            let next = remaining
                .iter()
                .copied()
                .min_by_key(|&candidate| {
                    (
                        input
                            .matrix
                            .travel_minutes(current, candidate)
                            .unwrap_or(u32::MAX),
                        input.problem.locations()[candidate]
                            .time_window()
                            .close()
                            .minutes(),
                        candidate,
                    )
                })
                .ok_or_else(|| SolverError::Failed("greedy route failed".to_owned()))?;
            remaining.remove(&next);
            visit_order.push(next);
            current = next;
        }
        visit_order.push(end);
        Ok(SolverSolution { visit_order })
    }
}

#[derive(Debug, Clone)]
pub struct ClusteredInitialRoute<S = ClusteredSolver> {
    solver: S,
}

impl Default for ClusteredInitialRoute {
    fn default() -> Self {
        Self {
            solver: ClusteredSolver::default(),
        }
    }
}

impl<S> ClusteredInitialRoute<S> {
    pub fn new(solver: S) -> Self {
        Self { solver }
    }
}

impl<S: RouteSolver + Send + Sync> InitialRouteGenerator for ClusteredInitialRoute<S> {
    fn generate(
        &self,
        input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        self.solver.solve(input)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RandomInitialRoute;

impl InitialRouteGenerator for RandomInitialRoute {
    fn generate(
        &self,
        input: SolverInput<'_>,
        rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        validate_generator_input(&input)?;
        let end = input.problem.end_location_index();
        if end == 0 {
            return Ok(SolverSolution {
                visit_order: vec![0],
            });
        }
        let mut intermediate: Vec<_> = (1..end).collect();
        intermediate.shuffle(rng);
        let mut visit_order = Vec::with_capacity(input.problem.locations().len());
        visit_order.push(input.problem.start_location_index());
        visit_order.extend(intermediate);
        visit_order.push(end);
        Ok(SolverSolution { visit_order })
    }
}

/// Default SA initializer: feasible clustered result first, then deterministic
/// MST double-tree, directed greedy, and finally a random permutation.
#[derive(Debug, Clone, Default)]
pub struct ClusteredMstGreedyInitialRoute {
    clustered: ClusteredInitialRoute,
    mst: MstDoubleTreeInitialRoute,
    greedy: GreedyInitialRoute,
    random: RandomInitialRoute,
}

impl InitialRouteGenerator for ClusteredMstGreedyInitialRoute {
    fn generate(
        &self,
        input: SolverInput<'_>,
        rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        validate_generator_input(&input)?;
        for generator in [
            &self.clustered as &dyn InitialRouteGenerator,
            &self.mst,
            &self.greedy,
        ] {
            match generator.generate(clone_input(&input), rng) {
                Ok(solution) => return Ok(solution),
                Err(SolverError::Cancelled) => return Err(SolverError::Cancelled),
                Err(_) => {}
            }
        }
        self.random.generate(input, rng)
    }
}

fn validate_generator_input(input: &SolverInput<'_>) -> Result<(), SolverError> {
    if input.cancellation.is_cancelled() {
        return Err(SolverError::Cancelled);
    }
    let locations = input.problem.locations().len();
    if input.matrix.size() != locations {
        return Err(SolverError::MatrixSizeMismatch {
            matrix: input.matrix.size(),
            locations,
        });
    }
    Ok(())
}

fn clone_input<'a>(input: &SolverInput<'a>) -> SolverInput<'a> {
    SolverInput {
        matrix: input.matrix,
        problem: input.problem,
        cancellation: input.cancellation,
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        api::OptimizeRouteRequest, cancellation::CancellationToken, domain::OptimizationProblem,
        solver::DefaultObjectivePolicy,
    };

    use super::*;
    use crate::solver::{evaluate_solution, ExactBitDpSolver};

    fn problem(location_count: usize) -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "mst-double-tree-test",
            "locations": (0..location_count).map(|index| serde_json::json!({
                "id": index.to_string(),
                "place_id": format!("p-{index}"),
                "open_time": "00:00",
                "close_time": "23:59",
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
