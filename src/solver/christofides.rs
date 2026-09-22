use std::{
    collections::BTreeSet,
    env, fmt,
    panic::{catch_unwind, AssertUnwindSafe},
    str::FromStr,
};

use integer_blossom::min_weight_perfect_matching;
use rand::RngCore;

use super::{
    minimum_spanning_tree, shortcut_euler_tour, AverageSymmetricDistance, InitialRouteGenerator,
    MstEdge, SolverError, SolverInput, SolverSolution, SymmetricDistanceStrategy,
};

pub const MAX_BIT_DP_MATCHING_VERTICES: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PerfectMatchingEdge {
    pub left: usize,
    pub right: usize,
    pub distance: u64,
}

/// Computes a minimum-weight perfect matching over `vertices`. `distances` is
/// indexed by positions in `vertices`, not by global location indices.
pub trait PerfectMatchingStrategy: Send + Sync {
    fn strategy_name(&self, _vertex_count: usize) -> &'static str {
        "custom"
    }

    fn minimum_weight_perfect_matching(
        &self,
        vertices: &[usize],
        distances: &[Vec<u64>],
    ) -> Result<Vec<PerfectMatchingEdge>, SolverError>;
}

/// Exact minimum-weight perfect matching using `dp[matched_mask]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitDpPerfectMatching {
    max_vertices: usize,
}

impl Default for BitDpPerfectMatching {
    fn default() -> Self {
        Self {
            max_vertices: MAX_BIT_DP_MATCHING_VERTICES,
        }
    }
}

impl BitDpPerfectMatching {
    pub fn new(max_vertices: usize) -> Result<Self, SolverError> {
        if max_vertices > MAX_BIT_DP_MATCHING_VERTICES {
            return Err(SolverError::InvalidConfiguration(format!(
                "bit-DP perfect matching limit must not exceed {MAX_BIT_DP_MATCHING_VERTICES}"
            )));
        }
        Ok(Self { max_vertices })
    }

    pub fn max_vertices(&self) -> usize {
        self.max_vertices
    }

    pub fn estimated_working_memory_bytes(&self, vertex_count: usize) -> Option<usize> {
        let state_count = 1_usize.checked_shl(vertex_count as u32)?;
        state_count.checked_mul(
            std::mem::size_of::<u64>() + std::mem::size_of::<Option<(usize, usize, usize)>>(),
        )
    }
}

impl PerfectMatchingStrategy for BitDpPerfectMatching {
    fn strategy_name(&self, _vertex_count: usize) -> &'static str {
        "bit_dp"
    }

    fn minimum_weight_perfect_matching(
        &self,
        vertices: &[usize],
        distances: &[Vec<u64>],
    ) -> Result<Vec<PerfectMatchingEdge>, SolverError> {
        validate_matching_problem(vertices, distances)?;
        let vertex_count = vertices.len();
        if vertex_count > self.max_vertices {
            return Err(SolverError::UnsupportedMatchingVertexCount {
                maximum: self.max_vertices,
                actual: vertex_count,
            });
        }
        if vertex_count == 0 {
            return Ok(Vec::new());
        }

        let state_count = 1_usize << vertex_count;
        let full_mask = state_count - 1;
        let mut dp = vec![u64::MAX; state_count];
        let mut predecessor = vec![None; state_count];
        dp[0] = 0;

        for mask in 0..state_count {
            if dp[mask] == u64::MAX || mask == full_mask {
                continue;
            }
            let unmatched = full_mask ^ mask;
            let first = unmatched.trailing_zeros() as usize;
            let first_bit = 1_usize << first;
            for (second, &pair_distance) in distances[first].iter().enumerate().skip(first + 1) {
                let second_bit = 1_usize << second;
                if unmatched & second_bit == 0 {
                    continue;
                }
                let next_mask = mask | first_bit | second_bit;
                let candidate = dp[mask]
                    .checked_add(pair_distance)
                    .ok_or_else(|| SolverError::Failed("matching cost overflow".to_owned()))?;
                if candidate < dp[next_mask] {
                    dp[next_mask] = candidate;
                    predecessor[next_mask] = Some((mask, first, second));
                }
            }
        }

        if dp[full_mask] == u64::MAX {
            return Err(SolverError::Failed(
                "perfect matching could not be constructed".to_owned(),
            ));
        }
        let mut matching = Vec::with_capacity(vertex_count / 2);
        let mut mask = full_mask;
        while mask != 0 {
            let (previous, left, right) = predecessor[mask].ok_or_else(|| {
                SolverError::Failed("perfect matching predecessor is missing".to_owned())
            })?;
            matching.push(PerfectMatchingEdge {
                left: vertices[left],
                right: vertices[right],
                distance: distances[left][right],
            });
            mask = previous;
        }
        matching.reverse();
        Ok(matching)
    }
}

/// Adapter around the maintained `integer-blossom` crate. The upstream solver
/// is deterministic and implements O(V^3) primal-dual weighted Blossom.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlossomPerfectMatching;

impl BlossomPerfectMatching {
    /// Size of the flattened i128 input buffer owned by this adapter. The
    /// upstream crate additionally reuses O(n^2) thread-local solver buffers.
    pub fn estimated_working_memory_bytes(&self, vertex_count: usize) -> Option<usize> {
        vertex_count
            .checked_mul(vertex_count)?
            .checked_mul(std::mem::size_of::<i128>())
    }
}

impl PerfectMatchingStrategy for BlossomPerfectMatching {
    fn strategy_name(&self, _vertex_count: usize) -> &'static str {
        "blossom"
    }

    fn minimum_weight_perfect_matching(
        &self,
        vertices: &[usize],
        distances: &[Vec<u64>],
    ) -> Result<Vec<PerfectMatchingEdge>, SolverError> {
        validate_matching_problem(vertices, distances)?;
        if vertices.is_empty() {
            return Ok(Vec::new());
        }
        let vertex_count = vertices.len();
        let costs: Vec<i128> = distances
            .iter()
            .flat_map(|row| row.iter().map(|&distance| i128::from(distance)))
            .collect();
        let mates = catch_unwind(AssertUnwindSafe(|| {
            min_weight_perfect_matching(&costs, vertex_count)
        }))
        .map_err(|_| {
            SolverError::PerfectMatchingFailed(
                "integer-blossom panicked while solving the matching".to_owned(),
            )
        })?;
        validate_blossom_mates(&mates, vertex_count)?;

        Ok((0..vertex_count)
            .filter_map(|left| {
                let right = mates[left];
                (left < right).then_some(PerfectMatchingEdge {
                    left: vertices[left],
                    right: vertices[right],
                    distance: distances[left][right],
                })
            })
            .collect())
    }
}

fn validate_blossom_mates(mates: &[usize], vertex_count: usize) -> Result<(), SolverError> {
    if mates.len() != vertex_count {
        return Err(SolverError::PerfectMatchingFailed(format!(
            "integer-blossom returned {} mates for {vertex_count} vertices",
            mates.len()
        )));
    }
    for (vertex, &mate) in mates.iter().enumerate() {
        if mate >= vertex_count || mate == vertex || mates.get(mate) != Some(&vertex) {
            return Err(SolverError::PerfectMatchingFailed(format!(
                "integer-blossom returned an invalid mate for vertex {vertex}"
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchingStrategyChoice {
    Auto,
    BitDp,
    Blossom,
}

impl FromStr for MatchingStrategyChoice {
    type Err = SolverError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            "bitdp" => Ok(Self::BitDp),
            "blossom" => Ok(Self::Blossom),
            _ => Err(SolverError::InvalidConfiguration(format!(
                "MATCHING_STRATEGY must be auto, bitdp, or blossom (actual {value})"
            ))),
        }
    }
}

impl fmt::Display for MatchingStrategyChoice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::BitDp => "bitdp",
            Self::Blossom => "blossom",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchingStrategyConfig {
    pub strategy: MatchingStrategyChoice,
    pub bit_dp_threshold: usize,
}

impl Default for MatchingStrategyConfig {
    fn default() -> Self {
        Self {
            strategy: MatchingStrategyChoice::Auto,
            bit_dp_threshold: MAX_BIT_DP_MATCHING_VERTICES,
        }
    }
}

impl MatchingStrategyConfig {
    pub fn new(
        strategy: MatchingStrategyChoice,
        bit_dp_threshold: usize,
    ) -> Result<Self, SolverError> {
        if bit_dp_threshold > MAX_BIT_DP_MATCHING_VERTICES {
            return Err(SolverError::InvalidConfiguration(format!(
                "MATCHING_BIT_DP_THRESHOLD must be between 0 and {MAX_BIT_DP_MATCHING_VERTICES}"
            )));
        }
        Ok(Self {
            strategy,
            bit_dp_threshold,
        })
    }

    /// Reads benchmark/debug overrides. Solvers still receive the resulting
    /// value explicitly and do not perform environment lookup while solving.
    pub fn from_env() -> Result<Self, SolverError> {
        let strategy = match env::var("MATCHING_STRATEGY") {
            Ok(value) => value.parse()?,
            Err(env::VarError::NotPresent) => MatchingStrategyChoice::Auto,
            Err(error) => {
                return Err(SolverError::InvalidConfiguration(format!(
                    "MATCHING_STRATEGY is invalid: {error}"
                )))
            }
        };
        let threshold = match env::var("MATCHING_BIT_DP_THRESHOLD") {
            Ok(value) => value.parse::<usize>().map_err(|_| {
                SolverError::InvalidConfiguration(format!(
                    "MATCHING_BIT_DP_THRESHOLD must be an integer between 0 and {MAX_BIT_DP_MATCHING_VERTICES}"
                ))
            })?,
            Err(env::VarError::NotPresent) => MAX_BIT_DP_MATCHING_VERTICES,
            Err(error) => {
                return Err(SolverError::InvalidConfiguration(format!(
                    "MATCHING_BIT_DP_THRESHOLD is invalid: {error}"
                )))
            }
        };
        Self::new(strategy, threshold)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoPerfectMatching {
    config: MatchingStrategyConfig,
    bit_dp: BitDpPerfectMatching,
    blossom: BlossomPerfectMatching,
}

impl Default for AutoPerfectMatching {
    fn default() -> Self {
        Self::new(MatchingStrategyConfig::default())
            .expect("the default matching configuration is valid")
    }
}

impl AutoPerfectMatching {
    pub fn new(config: MatchingStrategyConfig) -> Result<Self, SolverError> {
        let config = MatchingStrategyConfig::new(config.strategy, config.bit_dp_threshold)?;
        Ok(Self {
            config,
            bit_dp: BitDpPerfectMatching::default(),
            blossom: BlossomPerfectMatching,
        })
    }

    pub fn config(&self) -> MatchingStrategyConfig {
        self.config
    }

    pub fn selected_strategy(&self, vertex_count: usize) -> MatchingStrategyChoice {
        match self.config.strategy {
            MatchingStrategyChoice::Auto if vertex_count <= self.config.bit_dp_threshold => {
                MatchingStrategyChoice::BitDp
            }
            MatchingStrategyChoice::Auto => MatchingStrategyChoice::Blossom,
            forced => forced,
        }
    }

    pub fn estimated_working_memory_bytes(&self, vertex_count: usize) -> Option<usize> {
        match self.selected_strategy(vertex_count) {
            MatchingStrategyChoice::BitDp => {
                self.bit_dp.estimated_working_memory_bytes(vertex_count)
            }
            MatchingStrategyChoice::Blossom => {
                self.blossom.estimated_working_memory_bytes(vertex_count)
            }
            MatchingStrategyChoice::Auto => unreachable!("auto is resolved above"),
        }
    }
}

impl PerfectMatchingStrategy for AutoPerfectMatching {
    fn strategy_name(&self, vertex_count: usize) -> &'static str {
        match self.selected_strategy(vertex_count) {
            MatchingStrategyChoice::BitDp => "bit_dp",
            MatchingStrategyChoice::Blossom => "blossom",
            MatchingStrategyChoice::Auto => unreachable!("auto is resolved above"),
        }
    }

    fn minimum_weight_perfect_matching(
        &self,
        vertices: &[usize],
        distances: &[Vec<u64>],
    ) -> Result<Vec<PerfectMatchingEdge>, SolverError> {
        match self.selected_strategy(vertices.len()) {
            MatchingStrategyChoice::BitDp => self
                .bit_dp
                .minimum_weight_perfect_matching(vertices, distances),
            MatchingStrategyChoice::Blossom => self
                .blossom
                .minimum_weight_perfect_matching(vertices, distances),
            MatchingStrategyChoice::Auto => unreachable!("auto is resolved above"),
        }
    }
}

fn validate_matching_problem(
    vertices: &[usize],
    distances: &[Vec<u64>],
) -> Result<(), SolverError> {
    if !vertices.len().is_multiple_of(2) {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching requires an even number of vertices".to_owned(),
        ));
    }
    if vertices.iter().copied().collect::<BTreeSet<_>>().len() != vertices.len() {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching vertices must be unique".to_owned(),
        ));
    }
    if distances.len() != vertices.len() || distances.iter().any(|row| row.len() != vertices.len())
    {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching distance matrix must match the vertex count".to_owned(),
        ));
    }
    for (left, row) in distances.iter().enumerate() {
        for (right, reverse_row) in distances.iter().enumerate().skip(left + 1) {
            if row[right] != reverse_row[left] {
                return Err(SolverError::InvalidConfiguration(
                    "perfect matching distances must be symmetric".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

pub fn odd_degree_vertices(
    node_count: usize,
    edges: &[MstEdge],
) -> Result<Vec<usize>, SolverError> {
    let mut degrees = vec![0_usize; node_count];
    for edge in edges {
        if edge.from >= node_count || edge.to >= node_count || edge.from == edge.to {
            return Err(SolverError::InvalidConfiguration(
                "graph edge contains an invalid endpoint".to_owned(),
            ));
        }
        degrees[edge.from] += 1;
        degrees[edge.to] += 1;
    }
    Ok(degrees
        .into_iter()
        .enumerate()
        .filter_map(|(vertex, degree)| (degree % 2 == 1).then_some(vertex))
        .collect())
}

/// Hierholzer traversal for a connected undirected Eulerian multigraph.
/// Parallel edges are represented by repeated `MstEdge` entries.
pub fn eulerian_multigraph_tour(
    node_count: usize,
    edges: &[MstEdge],
    root: usize,
) -> Result<Vec<usize>, SolverError> {
    if node_count == 0 || root >= node_count {
        return Err(SolverError::InvalidConfiguration(
            "Euler tour root must be inside a non-empty graph".to_owned(),
        ));
    }
    let mut adjacency = vec![Vec::<(usize, u64, usize)>::new(); node_count];
    let mut degrees = vec![0_usize; node_count];
    for (edge_id, edge) in edges.iter().enumerate() {
        if edge.from >= node_count || edge.to >= node_count || edge.from == edge.to {
            return Err(SolverError::InvalidConfiguration(
                "Euler graph edge contains an invalid endpoint".to_owned(),
            ));
        }
        adjacency[edge.from].push((edge.to, edge.distance, edge_id));
        adjacency[edge.to].push((edge.from, edge.distance, edge_id));
        degrees[edge.from] += 1;
        degrees[edge.to] += 1;
    }
    if degrees.iter().any(|degree| degree % 2 != 0) {
        return Err(SolverError::InvalidConfiguration(
            "Euler graph must have even degree at every vertex".to_owned(),
        ));
    }
    for neighbors in &mut adjacency {
        neighbors.sort_by_key(|&(vertex, distance, edge_id)| (distance, vertex, edge_id));
    }

    let mut used_edges = vec![false; edges.len()];
    let mut cursors = vec![0_usize; node_count];
    let mut stack = vec![root];
    let mut reverse_tour = Vec::with_capacity(edges.len() + 1);
    while let Some(&vertex) = stack.last() {
        while cursors[vertex] < adjacency[vertex].len()
            && used_edges[adjacency[vertex][cursors[vertex]].2]
        {
            cursors[vertex] += 1;
        }
        if let Some(&(next, _, edge_id)) = adjacency[vertex].get(cursors[vertex]) {
            cursors[vertex] += 1;
            used_edges[edge_id] = true;
            stack.push(next);
        } else {
            reverse_tour.push(vertex);
            stack.pop();
        }
    }
    let mut reached_vertices = vec![false; node_count];
    for &vertex in &reverse_tour {
        reached_vertices[vertex] = true;
    }
    if used_edges.iter().any(|used| !used)
        || reached_vertices.iter().any(|reached| !reached)
        || reverse_tour.len() != edges.len() + 1
    {
        return Err(SolverError::InvalidConfiguration(
            "Euler graph must be connected".to_owned(),
        ));
    }
    reverse_tour.reverse();
    Ok(reverse_tour)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChristofidesResult {
    pub symmetric_distance_strategy: String,
    pub matching_strategy: String,
    pub mst_edges: Vec<MstEdge>,
    pub odd_vertices: Vec<usize>,
    pub matching_edges: Vec<PerfectMatchingEdge>,
    pub eulerian_edges: Vec<MstEdge>,
    pub euler_tour: Vec<usize>,
    pub solution: SolverSolution,
}

#[derive(Debug, Clone)]
pub struct ChristofidesInitialRoute<S = AverageSymmetricDistance, P = AutoPerfectMatching> {
    symmetric_distance: S,
    perfect_matching: P,
}

impl Default for ChristofidesInitialRoute {
    fn default() -> Self {
        Self {
            symmetric_distance: AverageSymmetricDistance,
            perfect_matching: AutoPerfectMatching::default(),
        }
    }
}

impl<S, P> ChristofidesInitialRoute<S, P> {
    pub fn new(symmetric_distance: S, perfect_matching: P) -> Self {
        Self {
            symmetric_distance,
            perfect_matching,
        }
    }
}

impl<S: SymmetricDistanceStrategy, P: PerfectMatchingStrategy> ChristofidesInitialRoute<S, P> {
    pub fn generate_detailed(
        &self,
        input: SolverInput<'_>,
    ) -> Result<ChristofidesResult, SolverError> {
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let node_count = input.problem.locations().len();
        if input.matrix.size() != node_count {
            return Err(SolverError::MatrixSizeMismatch {
                matrix: input.matrix.size(),
                locations: node_count,
            });
        }
        let mst_edges = minimum_spanning_tree(node_count, |left, right| {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            self.symmetric_distance
                .symmetric_distance(input.matrix, left, right)
        })?;
        let odd_vertices = odd_degree_vertices(node_count, &mst_edges)?;
        debug_assert_eq!(odd_vertices.len() % 2, 0);
        let mut odd_distances = vec![vec![0_u64; odd_vertices.len()]; odd_vertices.len()];
        for left in 0..odd_vertices.len() {
            for right in left + 1..odd_vertices.len() {
                let distance = self.symmetric_distance.symmetric_distance(
                    input.matrix,
                    odd_vertices[left],
                    odd_vertices[right],
                )?;
                odd_distances[left][right] = distance;
                odd_distances[right][left] = distance;
            }
        }
        let matching_edges = self
            .perfect_matching
            .minimum_weight_perfect_matching(&odd_vertices, &odd_distances)?;
        validate_matching_result(&odd_vertices, &matching_edges)?;

        let mut eulerian_edges = mst_edges.clone();
        eulerian_edges.extend(matching_edges.iter().map(|edge| MstEdge {
            from: edge.left,
            to: edge.right,
            distance: edge.distance,
        }));
        let euler_tour = eulerian_multigraph_tour(node_count, &eulerian_edges, 0)?;
        let mut visit_order = shortcut_euler_tour(node_count, &euler_tour)?;
        move_fixed_destination_to_end(&mut visit_order, input.problem.end_location_index())?;

        Ok(ChristofidesResult {
            symmetric_distance_strategy: self.symmetric_distance.name().to_owned(),
            matching_strategy: self
                .perfect_matching
                .strategy_name(odd_vertices.len())
                .to_owned(),
            mst_edges,
            odd_vertices,
            matching_edges,
            eulerian_edges,
            euler_tour,
            solution: SolverSolution { visit_order },
        })
    }
}

impl<S: SymmetricDistanceStrategy, P: PerfectMatchingStrategy> InitialRouteGenerator
    for ChristofidesInitialRoute<S, P>
{
    fn generate(
        &self,
        input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        self.generate_detailed(input).map(|result| result.solution)
    }
}

fn validate_matching_result(
    odd_vertices: &[usize],
    matching: &[PerfectMatchingEdge],
) -> Result<(), SolverError> {
    if matching.len() * 2 != odd_vertices.len() {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching does not cover every odd vertex".to_owned(),
        ));
    }
    let expected: BTreeSet<_> = odd_vertices.iter().copied().collect();
    let mut actual = BTreeSet::new();
    for edge in matching {
        if edge.left == edge.right || !actual.insert(edge.left) || !actual.insert(edge.right) {
            return Err(SolverError::InvalidConfiguration(
                "perfect matching must use every odd vertex exactly once".to_owned(),
            ));
        }
    }
    if actual != expected {
        return Err(SolverError::InvalidConfiguration(
            "perfect matching contains a non-odd vertex".to_owned(),
        ));
    }
    Ok(())
}

fn move_fixed_destination_to_end(
    visit_order: &mut Vec<usize>,
    destination: usize,
) -> Result<(), SolverError> {
    if visit_order.len() <= 1 {
        return Ok(());
    }
    let position = visit_order
        .iter()
        .position(|&vertex| vertex == destination)
        .ok_or(SolverError::InvalidVisitOrder)?;
    visit_order.remove(position);
    visit_order.push(destination);
    Ok(())
}

#[cfg(test)]
mod tests {
    use rand::{rngs::StdRng, SeedableRng};

    use crate::{
        api::OptimizeRouteRequest, cancellation::CancellationToken, domain::OptimizationProblem,
        matrix::TravelTimeMatrix,
    };

    use super::*;
    use crate::solver::{
        evaluate_solution, DefaultObjectivePolicy, ExactBitDpSolver, MixedNeighborhoodStrategy,
        SimulatedAnnealingConfig, SimulatedAnnealingSolver,
    };

    fn problem(location_count: usize) -> OptimizationProblem {
        let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
            "job_id": "christofides-test",
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
