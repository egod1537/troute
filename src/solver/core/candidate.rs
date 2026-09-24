use std::time::Duration;

use super::{SolutionMetrics, SolverSolution};

/// Common result returned by every orchestrated strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverCandidate {
    pub strategy: String,
    pub route: Option<SolverSolution>,
    pub feasible: bool,
    pub objective_score: Option<SolutionMetrics>,
    pub elapsed: Duration,
    pub metadata: SolverCandidateMetadata,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SolverCandidateMetadata {
    pub state_count: Option<usize>,
    pub frontier_state_count: Option<usize>,
    pub frontier_cell_count: Option<usize>,
    pub cluster_count: Option<usize>,
    pub cluster_sizes: Option<Vec<usize>>,
    pub cluster_strategy: Option<String>,
    pub cluster_order_strategy: Option<String>,
    pub cluster_order: Option<Vec<usize>>,
    pub cluster_details: Option<Vec<ClusterDiagnostic>>,
    pub score_before_improvement: Option<u64>,
    pub score_after_improvement: Option<u64>,
    pub improvement_strategy: Option<String>,
    pub swap_enabled: Option<bool>,
    pub relocate_enabled: Option<bool>,
    pub two_opt_enabled: Option<bool>,
    pub symmetric_distance_strategy: Option<String>,
    pub mst_cost: Option<u64>,
    pub mst_edge_count: Option<usize>,
    pub mst_edges: Option<Vec<MstEdgeDiagnostic>>,
    pub euler_tour: Option<Vec<String>>,
    pub shortcut_route: Option<Vec<String>>,
    pub odd_vertices: Option<Vec<String>>,
    pub odd_vertex_count: Option<usize>,
    pub matching_strategy: Option<String>,
    pub matching_cost: Option<u64>,
    pub matching_pairs: Option<Vec<MatchingPairDiagnostic>>,
    pub initial_strategy: Option<String>,
    pub initial_route: Option<Vec<String>>,
    pub final_route: Option<Vec<String>>,
    pub initial_score: Option<u64>,
    pub final_score: Option<u64>,
    pub initial_temperature: Option<String>,
    pub final_temperature: Option<String>,
    pub cooling_rate: Option<String>,
    pub swap_move_count: Option<u64>,
    pub relocate_move_count: Option<u64>,
    pub two_opt_move_count: Option<u64>,
    pub accepted_worse_moves: Option<u64>,
    pub infeasible_candidates: Option<u64>,
    pub accepted_infeasible_moves: Option<u64>,
    pub best_feasible: Option<bool>,
    pub iteration_count: Option<u64>,
    pub accepted_moves: Option<u64>,
    pub improved_moves: Option<u64>,
    pub seed: Option<u64>,
    pub timed_out: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClusterDiagnostic {
    /// Stable, one-based cluster number assigned before cluster ordering.
    pub cluster: usize,
    pub members: Vec<String>,
    /// Exact route inside this cluster before boundary improvement.
    pub route: Vec<String>,
    pub entry: Option<String>,
    pub exit: Option<String>,
    pub state_count: usize,
    pub frontier_state_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MstEdgeDiagnostic {
    pub from: String,
    pub to: String,
    pub distance: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchingPairDiagnostic {
    pub left: String,
    pub right: String,
    pub distance: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverDiagnostics {
    pub selected_strategy: String,
    pub candidates: Vec<SolverCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolverRunResult {
    pub solution: SolverSolution,
    pub diagnostics: Option<SolverDiagnostics>,
}
