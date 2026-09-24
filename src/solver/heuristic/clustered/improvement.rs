use std::cmp::Ordering;

use crate::solver::{
    evaluate_solution, ObjectivePolicy, SolutionMetrics, SolverError, SolverInput, SolverSolution,
};

pub trait LocalImprovementStrategy: Send + Sync {
    fn name(&self) -> &'static str {
        "custom"
    }

    fn enabled_operations(&self) -> LocalImprovementOperations {
        LocalImprovementOperations::default()
    }

    fn improve(
        &self,
        route: &mut Vec<usize>,
        cluster_boundaries: &[usize],
        input: SolverInput<'_>,
        objective: &dyn ObjectivePolicy,
    ) -> Result<LocalImprovementResult, SolverError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalImprovementResult {
    pub metrics: SolutionMetrics,
    pub accepted_moves: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalImprovementOperations {
    pub swap: bool,
    pub relocate: bool,
    pub two_opt: bool,
}

/// Tries swaps between the last member of one cluster and the first member of
/// the next cluster. A move is accepted only when the shared objective says it
/// is strictly better and the complete route remains feasible.
#[derive(Debug, Clone, Copy, Default)]
pub struct BoundarySwapLocalImprovement;

impl LocalImprovementStrategy for BoundarySwapLocalImprovement {
    fn name(&self) -> &'static str {
        "boundary_swap"
    }

    fn enabled_operations(&self) -> LocalImprovementOperations {
        LocalImprovementOperations {
            swap: true,
            relocate: false,
            two_opt: false,
        }
    }

    fn improve(
        &self,
        route: &mut Vec<usize>,
        cluster_boundaries: &[usize],
        input: SolverInput<'_>,
        objective: &dyn ObjectivePolicy,
    ) -> Result<LocalImprovementResult, SolverError> {
        let mut metrics = evaluate_solution(
            objective,
            SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: input.cancellation,
            },
            &SolverSolution {
                visit_order: route.clone(),
            },
        )?;
        let mut accepted_moves = 0;
        for &boundary in cluster_boundaries {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            if boundary == 0 || boundary + 1 >= route.len() - 1 {
                continue;
            }
            route.swap(boundary, boundary + 1);
            let candidate_solution = SolverSolution {
                visit_order: route.clone(),
            };
            let candidate = evaluate_solution(
                objective,
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                &candidate_solution,
            );
            match candidate {
                Ok(candidate) if objective.compare(&candidate, &metrics) == Ordering::Less => {
                    metrics = candidate;
                    accepted_moves += 1;
                }
                Ok(_) | Err(SolverError::NoFeasibleRoute) => route.swap(boundary, boundary + 1),
                Err(error) => return Err(error),
            }
        }
        Ok(LocalImprovementResult {
            metrics,
            accepted_moves,
        })
    }
}
