use std::time::{Duration, Instant};

use rand::{rngs::StdRng, Rng, RngCore, SeedableRng};

use crate::{
    domain::TimeOfDay,
    solver::{
        evaluate_solution, evaluate_solution_with_penalties, ClusteredMstGreedyInitialRoute,
        ClusteredSolver, DefaultObjectivePolicy, InitialRouteGenerator, ObjectivePolicy,
        RandomInitialRoute, RouteSolver, SolutionMetrics, SolverError, SolverInput, SolverSolution,
    },
};

use super::{
    MixedNeighborhoodStrategy, NeighborhoodMove, NeighborhoodStrategy, SimulatedAnnealingConfig,
};

pub trait InitialRouteStrategy: Send + Sync {
    fn initial_solution(
        &self,
        input: SolverInput<'_>,
        rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError>;
}

/// Uses an already evaluated route as the starting point for another anytime
/// improvement chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedInitialRoute {
    pub solution: SolverSolution,
}

impl InitialRouteStrategy for FixedInitialRoute {
    fn initial_solution(
        &self,
        input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        Ok(self.solution.clone())
    }
}

impl<T: InitialRouteGenerator> InitialRouteStrategy for T {
    fn initial_solution(
        &self,
        input: SolverInput<'_>,
        rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        self.generate(input, rng)
    }
}

/// Backward-compatible name for the default initializer, which now includes
/// MST double-tree between clustered and greedy generation.
pub type ClusteredThenGreedyInitialStrategy = ClusteredMstGreedyInitialRoute;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimulatedAnnealingStats {
    pub iterations: u64,
    pub accepted_moves: u64,
    pub improved_moves: u64,
    pub swap_moves: u64,
    pub relocate_moves: u64,
    pub two_opt_moves: u64,
    pub accepted_worse_moves: u64,
    pub infeasible_candidates: u64,
    pub accepted_infeasible_moves: u64,
    pub initial_temperature: f64,
    pub final_temperature: f64,
    pub cooling_rate: f64,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SimulatedAnnealingResult {
    pub initial_solution: SolverSolution,
    pub initial_metrics: Option<SolutionMetrics>,
    pub solution: SolverSolution,
    pub metrics: SolutionMetrics,
    pub stats: SimulatedAnnealingStats,
}

#[derive(Debug, Clone)]
pub struct SimulatedAnnealingSolver<
    I = ClusteredThenGreedyInitialStrategy,
    N = MixedNeighborhoodStrategy,
    O = DefaultObjectivePolicy,
> {
    config: SimulatedAnnealingConfig,
    initial_strategy: I,
    neighborhood_strategy: N,
    objective: O,
}

impl Default for SimulatedAnnealingSolver {
    fn default() -> Self {
        Self {
            config: SimulatedAnnealingConfig::default(),
            initial_strategy: ClusteredThenGreedyInitialStrategy::default(),
            neighborhood_strategy: MixedNeighborhoodStrategy,
            objective: DefaultObjectivePolicy,
        }
    }
}

impl SimulatedAnnealingSolver {
    pub fn with_config(config: SimulatedAnnealingConfig) -> Result<Self, SolverError> {
        Ok(Self {
            config: config.validate()?,
            ..Self::default()
        })
    }
}

impl<I, N, O> SimulatedAnnealingSolver<I, N, O> {
    pub fn new(
        config: SimulatedAnnealingConfig,
        initial_strategy: I,
        neighborhood_strategy: N,
        objective: O,
    ) -> Result<Self, SolverError> {
        Ok(Self {
            config: config.validate()?,
            initial_strategy,
            neighborhood_strategy,
            objective,
        })
    }
}

impl<I: InitialRouteStrategy, N: NeighborhoodStrategy, O: ObjectivePolicy>
    SimulatedAnnealingSolver<I, N, O>
{
    pub fn solve_detailed(
        &self,
        input: SolverInput<'_>,
    ) -> Result<SimulatedAnnealingResult, SolverError> {
        let config = self.config.validate()?;
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        if input.matrix.size() != input.problem.locations().len() {
            return Err(SolverError::MatrixSizeMismatch {
                matrix: input.matrix.size(),
                locations: input.problem.locations().len(),
            });
        }
        let mut rng = match config.seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            None => StdRng::from_entropy(),
        };
        let started = Instant::now();
        let mut current = self.initial_strategy.initial_solution(
            SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: input.cancellation,
            },
            &mut rng,
        )?;
        if !is_permutation_with_fixed_endpoints(&current, input.problem.locations().len()) {
            current = RandomInitialRoute.generate(
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                &mut rng,
            )?;
        }
        let mut current_evaluation = evaluate_solution_with_penalties(
            &self.objective,
            SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: input.cancellation,
            },
            &current,
            config.penalties,
        )?;
        let initial_solution = current.clone();
        let initial_metrics = current_evaluation.feasible_metrics;
        let mut best_feasible = current_evaluation
            .feasible_metrics
            .map(|metrics| (current.clone(), metrics));
        let mut temperature = config.initial_temperature;
        let time_limit = config.time_limit_ms.map(Duration::from_millis);
        let mut iterations = 0_u64;
        let mut accepted_moves = 0_u64;
        let mut improved_moves = 0_u64;
        let mut swap_moves = 0_u64;
        let mut relocate_moves = 0_u64;
        let mut two_opt_moves = 0_u64;
        let mut accepted_worse_moves = 0_u64;
        let mut infeasible_candidates = 0_u64;
        let mut accepted_infeasible_moves = 0_u64;
        let mut interrupted = false;

        while optional_limit_allows(config.iteration_limit, iterations)
            && optional_duration_allows(time_limit, started.elapsed())
            && temperature >= config.minimum_temperature
        {
            if input.cancellation.is_cancelled() {
                interrupted = true;
                break;
            }
            let Some((candidate, movement)) = self
                .neighborhood_strategy
                .neighbor(&current.visit_order, &mut rng)
            else {
                break;
            };
            match movement {
                NeighborhoodMove::Swap { .. } => swap_moves += 1,
                NeighborhoodMove::Relocate { .. } => relocate_moves += 1,
                NeighborhoodMove::TwoOpt { .. } => two_opt_moves += 1,
            }
            let candidate = SolverSolution {
                visit_order: candidate,
            };
            let candidate_evaluation = match evaluate_solution_with_penalties(
                &self.objective,
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                &candidate,
                config.penalties,
            ) {
                Ok(evaluation) => evaluation,
                Err(SolverError::Cancelled) => {
                    interrupted = true;
                    break;
                }
                Err(error) => return Err(error),
            };
            if !candidate_evaluation.feasible {
                infeasible_candidates += 1;
            }
            if let Some(candidate_metrics) = candidate_evaluation.feasible_metrics {
                let improves_best = match best_feasible.as_ref() {
                    Some((_, best_metrics)) => self
                        .objective
                        .compare(&candidate_metrics, best_metrics)
                        .is_lt(),
                    None => true,
                };
                if improves_best {
                    best_feasible = Some((candidate.clone(), candidate_metrics));
                }
            }

            let delta = candidate_evaluation.energy - current_evaluation.energy;
            let random_unit = rng.gen::<f64>();
            if simulated_annealing_accept(delta, temperature, random_unit) {
                accepted_moves += 1;
                if delta < 0.0 {
                    improved_moves += 1;
                } else if delta > 0.0 {
                    accepted_worse_moves += 1;
                }
                if !candidate_evaluation.feasible {
                    accepted_infeasible_moves += 1;
                }
                current = candidate;
                current_evaluation = candidate_evaluation;
            }
            iterations += 1;
            temperature *= config.cooling_rate;
        }

        let Some((solution, metrics)) = best_feasible else {
            return Err(SolverError::NoFeasibleRoute);
        };
        let verified = if interrupted || input.cancellation.is_cancelled() {
            metrics
        } else {
            evaluate_solution(
                &self.objective,
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                &solution,
            )?
        };
        debug_assert_eq!(verified, metrics);
        Ok(SimulatedAnnealingResult {
            initial_solution,
            initial_metrics,
            solution,
            metrics: verified,
            stats: SimulatedAnnealingStats {
                iterations,
                accepted_moves,
                improved_moves,
                swap_moves,
                relocate_moves,
                two_opt_moves,
                accepted_worse_moves,
                infeasible_candidates,
                accepted_infeasible_moves,
                initial_temperature: config.initial_temperature,
                final_temperature: temperature,
                cooling_rate: config.cooling_rate,
                elapsed: started.elapsed(),
            },
        })
    }
}

impl<I: InitialRouteStrategy, N: NeighborhoodStrategy, O: ObjectivePolicy> RouteSolver
    for SimulatedAnnealingSolver<I, N, O>
{
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        self.solve_detailed(input).map(|result| result.solution)
    }

    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        let metrics = evaluate_solution(&self.objective, input, solution)?;
        crate::solver::selected_start_time(input.problem, &metrics)
    }
}

/// Runtime-selectable large-N solver used by the server's automatic threshold
/// solver. Both variants consume only the already-built matrix.
#[derive(Debug, Clone)]
pub enum LargeRouteSolver {
    Clustered(ClusteredSolver),
    SimulatedAnnealing(SimulatedAnnealingSolver),
}

impl RouteSolver for LargeRouteSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        match self {
            Self::Clustered(solver) => solver.solve(input),
            Self::SimulatedAnnealing(solver) => solver.solve(input),
        }
    }

    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        match self {
            Self::Clustered(solver) => solver.selected_start_time(input, solution),
            Self::SimulatedAnnealing(solver) => solver.selected_start_time(input, solution),
        }
    }
}

fn optional_limit_allows(limit: Option<u64>, current: u64) -> bool {
    match limit {
        Some(limit) => current < limit,
        None => true,
    }
}

fn optional_duration_allows(limit: Option<Duration>, elapsed: Duration) -> bool {
    match limit {
        Some(limit) => elapsed < limit,
        None => true,
    }
}

fn is_permutation_with_fixed_endpoints(solution: &SolverSolution, location_count: usize) -> bool {
    if solution.visit_order.len() != location_count
        || solution.visit_order.first() != Some(&0)
        || solution.visit_order.last() != location_count.checked_sub(1).as_ref()
    {
        return false;
    }
    let mut seen = vec![false; location_count];
    solution.visit_order.iter().all(|&location| {
        if location >= location_count || seen[location] {
            false
        } else {
            seen[location] = true;
            true
        }
    })
}

pub fn simulated_annealing_accept(delta: f64, temperature: f64, random_unit: f64) -> bool {
    delta <= 0.0
        || (temperature > 0.0
            && (0.0..1.0).contains(&random_unit)
            && random_unit < (-delta / temperature).exp())
}
