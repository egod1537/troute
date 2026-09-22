use std::time::{Duration, Instant};

use rand::{rngs::StdRng, Rng, RngCore, SeedableRng};

use crate::domain::TimeOfDay;

use super::{
    evaluate_solution, evaluate_solution_with_penalties, ClusteredMstGreedyInitialRoute,
    ClusteredSolver, DefaultObjectivePolicy, InfeasiblePenaltyWeights, InitialRouteGenerator,
    ObjectivePolicy, RandomInitialRoute, RouteSolver, SolutionMetrics, SolverError, SolverInput,
    SolverSolution, TIME_SLOT_MINUTES,
};

pub trait InitialRouteStrategy: Send + Sync {
    fn initial_solution(
        &self,
        input: SolverInput<'_>,
        rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError>;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeighborhoodMove {
    Swap { left: usize, right: usize },
    Relocate { from: usize, to: usize },
    TwoOpt { start: usize, end: usize },
}

pub fn apply_neighborhood_move(route: &mut Vec<usize>, movement: NeighborhoodMove) -> bool {
    let is_interior = |index: usize| index > 0 && index + 1 < route.len();
    match movement {
        NeighborhoodMove::Swap { left, right }
            if left != right && is_interior(left) && is_interior(right) =>
        {
            route.swap(left, right);
            true
        }
        NeighborhoodMove::Relocate { from, to }
            if from != to && is_interior(from) && is_interior(to) =>
        {
            let location = route.remove(from);
            route.insert(to, location);
            true
        }
        NeighborhoodMove::TwoOpt { start, end }
            if start < end && is_interior(start) && is_interior(end) =>
        {
            route[start..=end].reverse();
            true
        }
        _ => false,
    }
}

pub trait NeighborhoodStrategy: Send + Sync {
    fn neighbor(
        &self,
        route: &[usize],
        rng: &mut dyn RngCore,
    ) -> Option<(Vec<usize>, NeighborhoodMove)>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MixedNeighborhoodStrategy;

impl NeighborhoodStrategy for MixedNeighborhoodStrategy {
    fn neighbor(
        &self,
        route: &[usize],
        rng: &mut dyn RngCore,
    ) -> Option<(Vec<usize>, NeighborhoodMove)> {
        if route.len() < 4 {
            return None;
        }
        let left = rng.gen_range(1..route.len() - 1);
        let mut right = rng.gen_range(1..route.len() - 1);
        while left == right {
            right = rng.gen_range(1..route.len() - 1);
        }
        let movement = match rng.gen_range(0..3) {
            0 => NeighborhoodMove::Swap { left, right },
            1 => NeighborhoodMove::Relocate {
                from: left,
                to: right,
            },
            _ => NeighborhoodMove::TwoOpt {
                start: left.min(right),
                end: left.max(right),
            },
        };
        let mut neighbor = route.to_vec();
        apply_neighborhood_move(&mut neighbor, movement).then_some((neighbor, movement))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimulatedAnnealingConfig {
    pub initial_temperature: f64,
    pub cooling_rate: f64,
    pub minimum_temperature: f64,
    pub iteration_limit: Option<u64>,
    pub time_limit_ms: Option<u64>,
    pub seed: Option<u64>,
    pub penalties: InfeasiblePenaltyWeights,
}

impl Default for SimulatedAnnealingConfig {
    fn default() -> Self {
        Self {
            initial_temperature: 1_000.0,
            cooling_rate: 0.995,
            minimum_temperature: 0.01,
            iteration_limit: Some(10_000),
            time_limit_ms: None,
            seed: None,
            penalties: InfeasiblePenaltyWeights::default(),
        }
    }
}

impl SimulatedAnnealingConfig {
    pub fn validate(self) -> Result<Self, SolverError> {
        if !self.initial_temperature.is_finite() || self.initial_temperature <= 0.0 {
            return Err(SolverError::InvalidConfiguration(
                "initial_temperature must be finite and positive".to_owned(),
            ));
        }
        if !self.cooling_rate.is_finite() || !(0.0..1.0).contains(&self.cooling_rate) {
            return Err(SolverError::InvalidConfiguration(
                "cooling_rate must be finite and between 0 and 1".to_owned(),
            ));
        }
        if !self.minimum_temperature.is_finite()
            || self.minimum_temperature <= 0.0
            || self.minimum_temperature >= self.initial_temperature
        {
            return Err(SolverError::InvalidConfiguration(
                "minimum_temperature must be positive and below initial_temperature".to_owned(),
            ));
        }
        if self.iteration_limit == Some(0) {
            return Err(SolverError::InvalidConfiguration(
                "iteration_limit must be positive when set".to_owned(),
            ));
        }
        if self.time_limit_ms == Some(0) {
            return Err(SolverError::InvalidConfiguration(
                "time_limit_ms must be positive when set".to_owned(),
            ));
        }
        self.penalties.validate()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimulatedAnnealingStats {
    pub iterations: u64,
    pub accepted_moves: u64,
    pub improved_moves: u64,
    pub accepted_worse_moves: u64,
    pub infeasible_candidates: u64,
    pub accepted_infeasible_moves: u64,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulatedAnnealingResult {
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
        let mut best_feasible = current_evaluation
            .feasible_metrics
            .map(|metrics| (current.clone(), metrics));
        let mut temperature = config.initial_temperature;
        let time_limit = config.time_limit_ms.map(Duration::from_millis);
        let mut iterations = 0_u64;
        let mut accepted_moves = 0_u64;
        let mut improved_moves = 0_u64;
        let mut accepted_worse_moves = 0_u64;
        let mut infeasible_candidates = 0_u64;
        let mut accepted_infeasible_moves = 0_u64;

        while optional_limit_allows(config.iteration_limit, iterations)
            && optional_duration_allows(time_limit, started.elapsed())
            && temperature >= config.minimum_temperature
        {
            if input.cancellation.is_cancelled() {
                return Err(SolverError::Cancelled);
            }
            let Some((candidate, _movement)) = self
                .neighborhood_strategy
                .neighbor(&current.visit_order, &mut rng)
            else {
                break;
            };
            let candidate = SolverSolution {
                visit_order: candidate,
            };
            let candidate_evaluation = evaluate_solution_with_penalties(
                &self.objective,
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                &candidate,
                config.penalties,
            )?;
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
        let verified = evaluate_solution(
            &self.objective,
            SolverInput {
                matrix: input.matrix,
                problem: input.problem,
                cancellation: input.cancellation,
            },
            &solution,
        )?;
        debug_assert_eq!(verified, metrics);
        Ok(SimulatedAnnealingResult {
            solution,
            metrics: verified,
            stats: SimulatedAnnealingStats {
                iterations,
                accepted_moves,
                improved_moves,
                accepted_worse_moves,
                infeasible_candidates,
                accepted_infeasible_moves,
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
        TimeOfDay::from_minutes(metrics.start_time_slot * TIME_SLOT_MINUTES as u16)
            .map_err(|error| SolverError::Failed(error.to_string()))
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

#[cfg(test)]
mod tests {
    use std::{sync::Mutex, thread};

    use crate::{
        api::OptimizeRouteRequest, cancellation::CancellationToken, domain::OptimizationProblem,
        matrix::TravelTimeMatrix, solver::MstDoubleTreeInitialRoute,
    };

    use super::*;

    fn problem(location_count: usize) -> OptimizationProblem {
        let locations: Vec<_> = (0..location_count)
            .map(|index| {
                serde_json::json!({
                    "id": index.to_string(),
                    "place_id": format!("place-{index}"),
                    "open_time": "09:00",
                    "close_time": "23:59",
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
        assert_eq!(first.metrics, second.metrics);
        assert_eq!(first.stats.iterations, second.stats.iterations);
        assert_eq!(first.stats.accepted_moves, second.stats.accepted_moves);
        assert_eq!(first.stats.improved_moves, second.stats.improved_moves);
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
                {"id":"0","place_id":"p0","open_time":"00:00","close_time":"23:59","stay_minutes":0},
                {"id":"1","place_id":"p1","open_time":"09:00","close_time":"09:30","stay_minutes":0},
                {"id":"2","place_id":"p2","open_time":"09:00","close_time":"23:59","stay_minutes":0},
                {"id":"3","place_id":"p3","open_time":"00:00","close_time":"23:59","stay_minutes":0}
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
