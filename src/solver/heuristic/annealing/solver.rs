use std::time::{Duration, Instant};

use rand::{rngs::StdRng, Rng, RngCore, SeedableRng};

use crate::{
    domain::TimeOfDay,
    solver::{
        evaluate_solution, evaluate_solution_with_penalties, ClusteredMstGreedyInitialRoute,
        ClusteredSolver, DefaultObjectivePolicy, InitialRouteGenerator, ObjectivePolicy,
        PenalizedRouteEvaluation, RandomInitialRoute, RouteSolver, SolutionMetrics, SolverError,
        SolverInput, SolverSolution,
    },
};

use super::{
    MixedNeighborhoodStrategy, NeighborhoodMove, NeighborhoodStrategy, SimulatedAnnealingConfig,
};

const MAX_SEARCH_SAMPLES: usize = 10;

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
    pub insert_optional_moves: u64,
    pub remove_optional_moves: u64,
    pub replace_optional_moves: u64,
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

struct AnnealingSearchState {
    current: SolverSolution,
    current_evaluation: PenalizedRouteEvaluation,
    best_feasible: Option<(SolverSolution, SolutionMetrics)>,
    temperature: f64,
    iterations: u64,
    accepted_moves: u64,
    improved_moves: u64,
    swap_moves: u64,
    relocate_moves: u64,
    two_opt_moves: u64,
    insert_optional_moves: u64,
    remove_optional_moves: u64,
    replace_optional_moves: u64,
    accepted_worse_moves: u64,
    infeasible_candidates: u64,
    accepted_infeasible_moves: u64,
    interrupted: bool,
}

impl AnnealingSearchState {
    fn new(
        current: SolverSolution,
        current_evaluation: PenalizedRouteEvaluation,
        initial_temperature: f64,
    ) -> Self {
        let best_feasible = current_evaluation
            .feasible_metrics
            .map(|metrics| (current.clone(), metrics));
        Self {
            current,
            current_evaluation,
            best_feasible,
            temperature: initial_temperature,
            iterations: 0,
            accepted_moves: 0,
            improved_moves: 0,
            swap_moves: 0,
            relocate_moves: 0,
            two_opt_moves: 0,
            insert_optional_moves: 0,
            remove_optional_moves: 0,
            replace_optional_moves: 0,
            accepted_worse_moves: 0,
            infeasible_candidates: 0,
            accepted_infeasible_moves: 0,
            interrupted: false,
        }
    }

    fn record_movement(&mut self, movement: NeighborhoodMove) {
        match movement {
            NeighborhoodMove::Swap { .. } => self.swap_moves += 1,
            NeighborhoodMove::Relocate { .. } => self.relocate_moves += 1,
            NeighborhoodMove::TwoOpt { .. } => self.two_opt_moves += 1,
            NeighborhoodMove::InsertOptional { .. } => self.insert_optional_moves += 1,
            NeighborhoodMove::RemoveOptional { .. } => self.remove_optional_moves += 1,
            NeighborhoodMove::ReplaceOptional { .. } => self.replace_optional_moves += 1,
        }
    }

    fn consider_candidate(
        &mut self,
        objective: &dyn ObjectivePolicy,
        candidate: SolverSolution,
        candidate_evaluation: PenalizedRouteEvaluation,
        random_unit: f64,
    ) {
        if !candidate_evaluation.feasible {
            self.infeasible_candidates += 1;
        }
        if let Some(candidate_metrics) = candidate_evaluation.feasible_metrics {
            let improves_best = match self.best_feasible.as_ref() {
                Some((_, best_metrics)) => {
                    objective.compare(&candidate_metrics, best_metrics).is_lt()
                }
                None => true,
            };
            if improves_best {
                self.best_feasible = Some((candidate.clone(), candidate_metrics));
            }
        }

        let delta = candidate_evaluation.energy - self.current_evaluation.energy;
        if simulated_annealing_accept(delta, self.temperature, random_unit) {
            self.accepted_moves += 1;
            if delta < 0.0 {
                self.improved_moves += 1;
            } else if delta > 0.0 {
                self.accepted_worse_moves += 1;
            }
            if !candidate_evaluation.feasible {
                self.accepted_infeasible_moves += 1;
            }
            self.current = candidate;
            self.current_evaluation = candidate_evaluation;
        }
    }

    fn finish_iteration(&mut self, cooling_rate: f64) {
        self.iterations += 1;
        self.temperature *= cooling_rate;
    }

    fn stats(
        &self,
        config: SimulatedAnnealingConfig,
        elapsed: Duration,
    ) -> SimulatedAnnealingStats {
        SimulatedAnnealingStats {
            iterations: self.iterations,
            accepted_moves: self.accepted_moves,
            improved_moves: self.improved_moves,
            swap_moves: self.swap_moves,
            relocate_moves: self.relocate_moves,
            two_opt_moves: self.two_opt_moves,
            insert_optional_moves: self.insert_optional_moves,
            remove_optional_moves: self.remove_optional_moves,
            replace_optional_moves: self.replace_optional_moves,
            accepted_worse_moves: self.accepted_worse_moves,
            infeasible_candidates: self.infeasible_candidates,
            accepted_infeasible_moves: self.accepted_infeasible_moves,
            initial_temperature: config.initial_temperature,
            final_temperature: self.temperature,
            cooling_rate: config.cooling_rate,
            elapsed,
        }
    }
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
        self.solve_recording(input, false).map(|(result, _)| result)
    }

    pub fn solve_detailed_with_samples(
        &self,
        input: SolverInput<'_>,
    ) -> Result<
        (
            SimulatedAnnealingResult,
            Vec<(SolverSolution, SolutionMetrics)>,
        ),
        SolverError,
    > {
        self.solve_recording(input, true)
    }

    fn solve_recording(
        &self,
        input: SolverInput<'_>,
        capture: bool,
    ) -> Result<
        (
            SimulatedAnnealingResult,
            Vec<(SolverSolution, SolutionMetrics)>,
        ),
        SolverError,
    > {
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
        if !is_structurally_valid_initial_solution(&current, input.problem) {
            current = RandomInitialRoute.generate(
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                &mut rng,
            )?;
        }
        input.problem.restore_order_locks(&mut current.visit_order);
        let current_evaluation = evaluate_solution_with_penalties(
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
        let mut state =
            AnnealingSearchState::new(current, current_evaluation, config.initial_temperature);
        let time_limit = config.time_limit_ms.map(Duration::from_millis);
        let mut samples: Vec<(SolverSolution, SolutionMetrics)> = Vec::new();

        while optional_limit_allows(config.iteration_limit, state.iterations)
            && optional_duration_allows(time_limit, started.elapsed())
            && state.temperature >= config.minimum_temperature
        {
            if input.cancellation.is_cancelled() {
                state.interrupted = true;
                break;
            }
            let Some((candidate, movement)) = self.neighborhood_strategy.neighbor_for_problem(
                &state.current.visit_order,
                input.problem,
                &mut rng,
            ) else {
                break;
            };
            if !input.problem.preserves_order_locks(&candidate) {
                state.finish_iteration(config.cooling_rate);
                continue;
            }
            state.record_movement(movement);
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
                    state.interrupted = true;
                    break;
                }
                Err(error) => return Err(error),
            };
            let random_unit = rng.gen::<f64>();
            let accepted_before = state.accepted_moves;
            state.consider_candidate(
                &self.objective,
                candidate,
                candidate_evaluation,
                random_unit,
            );
            if capture
                && state.accepted_moves > accepted_before
                && (samples.len() < MAX_SEARCH_SAMPLES || state.accepted_moves % 16 == 0)
                && state.current.visit_order != initial_solution.visit_order
            {
                if let Some(metrics) = state.current_evaluation.feasible_metrics {
                    if !samples
                        .iter()
                        .any(|(route, _)| route.visit_order == state.current.visit_order)
                    {
                        let sample = (state.current.clone(), metrics);
                        if samples.len() < MAX_SEARCH_SAMPLES {
                            samples.push(sample);
                        } else {
                            samples[MAX_SEARCH_SAMPLES - 1] = sample;
                        }
                    }
                }
            }
            state.finish_iteration(config.cooling_rate);
        }

        let Some((solution, metrics)) = state.best_feasible.take() else {
            return Err(SolverError::NoFeasibleRoute);
        };
        let verified = if state.interrupted || input.cancellation.is_cancelled() {
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
        Ok((
            SimulatedAnnealingResult {
                initial_solution,
                initial_metrics,
                solution,
                metrics: verified,
                stats: state.stats(config, started.elapsed()),
            },
            samples,
        ))
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

fn is_structurally_valid_initial_solution(
    solution: &SolverSolution,
    problem: &crate::domain::OptimizationProblem,
) -> bool {
    let location_count = problem.locations().len();
    if (problem.visit_policy() == crate::domain::VisitPolicy::AllRequired
        && solution.visit_order.len() != location_count)
        || solution.visit_order.len() < 2
        || solution.visit_order.first() != Some(&0)
        || solution.visit_order.last() != location_count.checked_sub(1).as_ref()
    {
        return false;
    }
    let mut seen = vec![false; location_count];
    let valid = solution.visit_order.iter().all(|&location| {
        if location >= location_count || seen[location] {
            false
        } else {
            seen[location] = true;
            true
        }
    });
    valid
        && problem
            .locations()
            .iter()
            .enumerate()
            .all(|(index, location)| !location.required() || seen[index])
}

pub fn simulated_annealing_accept(delta: f64, temperature: f64, random_unit: f64) -> bool {
    delta <= 0.0
        || (temperature > 0.0
            && (0.0..1.0).contains(&random_unit)
            && random_unit < (-delta / temperature).exp())
}
