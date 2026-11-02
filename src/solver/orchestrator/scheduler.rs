use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    solver::{
        ArmSelection, InstanceFeatures, InstancePolicyModel, InstancePolicySelection,
        PortfolioPolicy, RestartableArmSelector, RuleBasedInstancePolicy, SolverError,
        SolverRegistry, SolverSolution, SolverTask, TaskId, TaskResult,
    },
};

use super::{
    diagnostics::{annealing_strategy, SaInitializer},
    initializer::{
        deterministic_sa_seed, InitializerSelectionPolicy, RoundRobinInitializerSelectionPolicy,
    },
    strategy::{duration_ms, resource_request, solver_id_for_strategy, OrchestratedStrategy},
    SolverOrchestratorConfig,
};

/// A task definition paired with the registered implementation that will run
/// it. The scheduler owns selection and construction; the executor only
/// consumes this value.
#[derive(Clone)]
pub struct ScheduledTask {
    pub task: SolverTask,
    strategy: Arc<dyn OrchestratedStrategy>,
}

impl ScheduledTask {
    pub fn new(task: SolverTask, strategy: Arc<dyn OrchestratedStrategy>) -> Self {
        Self { task, strategy }
    }

    pub fn strategy(&self) -> &dyn OrchestratedStrategy {
        self.strategy.as_ref()
    }

    pub fn strategy_name(&self) -> &str {
        self.strategy.name()
    }
}

/// Policy boundary for selecting tasks and reacting to completed runs.
/// Implementations may create follow-up tasks from observations, but never
/// execute solver code themselves.
pub trait TaskScheduler {
    fn next_tasks(&mut self) -> Result<Vec<ScheduledTask>, SolverError>;

    fn observe(&mut self, result: &mut TaskResult) -> Result<(), SolverError>;

    fn cancel(&mut self) {}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSchedulerSummary {
    pub baseline_elapsed: Duration,
    pub sa_elapsed: Duration,
    pub sa_run_count: u64,
    pub termination_reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchedulerPhase {
    Exact,
    Baselines,
    Annealing,
    Done,
}

struct PendingAnnealingRun {
    task_id: TaskId,
    initializer: SaInitializer,
    seed: u64,
    arm_selection: Option<ArmSelection>,
}

/// Per-solve scheduler that preserves the legacy exact-first, baseline batch,
/// and restartable SA behavior while keeping those choices out of the
/// orchestrator and executor.
pub(super) struct PortfolioTaskScheduler {
    config: SolverOrchestratorConfig,
    strategies: Vec<Arc<dyn OrchestratedStrategy>>,
    instance_selection: Option<InstancePolicySelection>,
    started: Instant,
    deadline: Instant,
    phase: SchedulerPhase,
    next_task_id: u64,
    exact_scheduled: bool,
    baselines_scheduled: bool,
    baseline_elapsed: Option<Duration>,
    sa_started: Option<Instant>,
    sa_run_count: u64,
    exhausted: bool,
    termination_reason: Option<&'static str>,
    best_solution: Option<SolverSolution>,
    adaptive_policy: Option<RestartableArmSelector>,
    pending_sa: Option<PendingAnnealingRun>,
}

impl PortfolioTaskScheduler {
    pub(super) fn new(
        config: &SolverOrchestratorConfig,
        strategies: &[Arc<dyn OrchestratedStrategy>],
        registry: &SolverRegistry,
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
        started: Instant,
    ) -> Result<Self, SolverError> {
        let instance_selection =
            (config.portfolio.policy == PortfolioPolicy::InstanceAware).then(|| {
                RuleBasedInstancePolicy.select(
                    &InstanceFeatures::extract(problem, matrix),
                    registry.descriptors(),
                )
            });
        let mut adaptive_config = config.portfolio.clone();
        if adaptive_config.policy == PortfolioPolicy::InstanceAware
            && adaptive_config.weights.is_empty()
        {
            let features = InstanceFeatures::extract(problem, matrix);
            if features.location_count >= 30 {
                adaptive_config.weights.extend([
                    ("greedy".to_owned(), 1.0),
                    ("mst_double_tree".to_owned(), 1.0),
                    ("christofides".to_owned(), 1.0),
                    ("clustered".to_owned(), 4.0),
                ]);
            } else if features.reachable_arc_ratio < 0.8 {
                adaptive_config
                    .weights
                    .extend([("greedy".to_owned(), 2.0), ("clustered".to_owned(), 3.0)]);
            }
        }
        let adaptive_policy = (config.portfolio.policy != PortfolioPolicy::Current)
            .then(|| {
                RestartableArmSelector::new(
                    adaptive_config,
                    vec![
                        SaInitializer::Greedy.label().to_owned(),
                        SaInitializer::MstDoubleTree.label().to_owned(),
                        SaInitializer::Christofides.label().to_owned(),
                        SaInitializer::Clustered.label().to_owned(),
                    ],
                )
                .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))
            })
            .transpose()?;
        Ok(Self {
            config: config.clone(),
            strategies: strategies
                .iter()
                .filter(|strategy| strategy.is_applicable_to(problem))
                .cloned()
                .collect(),
            instance_selection,
            started,
            deadline: started + config.total_budget,
            phase: SchedulerPhase::Exact,
            next_task_id: 1,
            exact_scheduled: false,
            baselines_scheduled: false,
            baseline_elapsed: None,
            sa_started: None,
            sa_run_count: 0,
            exhausted: false,
            termination_reason: None,
            best_solution: None,
            adaptive_policy,
            pending_sa: None,
        })
    }

    pub(super) fn execution_deadline(&self) -> Instant {
        self.deadline
            .checked_sub(self.config.deadline_safety_margin)
            .unwrap_or(self.deadline)
    }

    pub(super) fn worker_count(&self) -> usize {
        self.config
            .max_concurrency
            .min(self.strategies.len())
            .max(1)
    }

    pub(super) fn summary(&self) -> TaskSchedulerSummary {
        let baseline_elapsed = self
            .baseline_elapsed
            .unwrap_or_else(|| self.started.elapsed());
        let sa_elapsed = self
            .sa_started
            .map_or(Duration::ZERO, |start| start.elapsed());
        let termination_reason = self.termination_reason.unwrap_or_else(|| {
            if self.exhausted {
                "exhausted"
            } else if Instant::now() >= self.execution_deadline() {
                "deadline"
            } else {
                "exhausted"
            }
        });
        TaskSchedulerSummary {
            baseline_elapsed,
            sa_elapsed,
            sa_run_count: self.sa_run_count,
            termination_reason: termination_reason.to_owned(),
        }
    }

    fn next_id(&mut self) -> TaskId {
        let id = TaskId::new(self.next_task_id);
        self.next_task_id = self.next_task_id.saturating_add(1);
        id
    }

    fn scheduled_task(
        &mut self,
        strategy: Arc<dyn OrchestratedStrategy>,
        timeout: Duration,
        seed: Option<u64>,
        warm_start: Option<SolverSolution>,
    ) -> ScheduledTask {
        let solver_id = solver_id_for_strategy(strategy.name());
        let mut task = SolverTask::new(
            self.next_id(),
            solver_id,
            timeout,
            resource_request(&self.config, strategy.name()),
        );
        task.seed = seed;
        task.warm_start = warm_start;
        ScheduledTask::new(task, strategy)
    }

    fn allowance(&self, cap: Duration) -> Option<Duration> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        (remaining > self.config.deadline_safety_margin)
            .then(|| cap.min(remaining - self.config.deadline_safety_margin))
    }

    fn next_annealing_task(&mut self) -> Result<Vec<ScheduledTask>, SolverError> {
        if !solver_enabled(&self.config, "sa")
            || !instance_policy_allows(self.instance_selection.as_ref(), "sa")
        {
            self.phase = SchedulerPhase::Done;
            self.termination_reason = Some("exhausted");
            return Ok(Vec::new());
        }
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining <= self.config.deadline_safety_margin {
            self.phase = SchedulerPhase::Done;
            self.termination_reason = Some("deadline");
            return Ok(Vec::new());
        }
        let usable = remaining - self.config.deadline_safety_margin;
        let policy_quantum = self.config.portfolio.quantum();
        if self.config.portfolio.policy != PortfolioPolicy::Current && usable < policy_quantum {
            self.phase = SchedulerPhase::Done;
            self.termination_reason = Some("deadline");
            return Ok(Vec::new());
        }
        let requested_chunk = if self.config.portfolio.policy == PortfolioPolicy::Current {
            self.config.sa_chunk
        } else {
            self.config.sa_chunk.max(policy_quantum)
        };
        let chunk = requested_chunk.min(usable);
        if chunk.is_zero() {
            self.phase = SchedulerPhase::Done;
            self.termination_reason = Some("deadline");
            return Ok(Vec::new());
        }
        let arm_selection = self
            .adaptive_policy
            .as_mut()
            .map(RestartableArmSelector::select)
            .transpose()
            .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))?;
        let initializer = match arm_selection
            .as_ref()
            .map(|selection| selection.arm_id.as_str())
        {
            Some("greedy") => SaInitializer::Greedy,
            Some("mst_double_tree") => SaInitializer::MstDoubleTree,
            Some("christofides") => SaInitializer::Christofides,
            Some("clustered") => SaInitializer::Clustered,
            Some(other) => {
                return Err(SolverError::InvalidConfiguration(format!(
                    "unknown SA policy arm: {other}"
                )))
            }
            None => RoundRobinInitializerSelectionPolicy
                .select(self.sa_run_count, self.best_solution.is_some()),
        };
        let warm_start = self.best_solution.clone();
        let round = self.sa_run_count / 4;
        let base_seed = self.config.sa_seeds[(round as usize) % self.config.sa_seeds.len()];
        let seed = deterministic_sa_seed(base_seed, initializer, round);
        let strategy = annealing_strategy(
            &self.config,
            initializer,
            warm_start.clone(),
            seed,
            self.sa_run_count + 1,
            duration_ms(chunk).saturating_sub(1).max(1),
        )?;
        let task_warm_start = (initializer == SaInitializer::GlobalBest)
            .then_some(warm_start)
            .flatten();
        let scheduled = self.scheduled_task(strategy, chunk, Some(seed), task_warm_start);
        self.pending_sa = Some(PendingAnnealingRun {
            task_id: scheduled.task.task_id,
            initializer,
            seed,
            arm_selection,
        });
        Ok(vec![scheduled])
    }
}

impl TaskScheduler for PortfolioTaskScheduler {
    fn next_tasks(&mut self) -> Result<Vec<ScheduledTask>, SolverError> {
        loop {
            match self.phase {
                SchedulerPhase::Exact => {
                    self.phase = SchedulerPhase::Baselines;
                    if self.exact_scheduled {
                        continue;
                    }
                    self.exact_scheduled = true;
                    let exact = self
                        .strategies
                        .iter()
                        .find(|strategy| strategy.name() == "exact_bit_dp");
                    let exact = exact.cloned().filter(|strategy| {
                        instance_policy_allows(self.instance_selection.as_ref(), strategy.name())
                    });
                    if let (Some(strategy), Some(timeout)) =
                        (exact, self.allowance(self.config.strategy_timeout))
                    {
                        return Ok(vec![self.scheduled_task(strategy, timeout, None, None)]);
                    }
                }
                SchedulerPhase::Baselines => {
                    self.phase = SchedulerPhase::Annealing;
                    if self.baselines_scheduled {
                        continue;
                    }
                    self.baselines_scheduled = true;
                    let Some(timeout) = self.allowance(self.config.strategy_timeout) else {
                        continue;
                    };
                    let strategies: Vec<_> = self
                        .strategies
                        .iter()
                        .filter(|strategy| {
                            strategy.name() != "exact_bit_dp"
                                && instance_policy_allows(
                                    self.instance_selection.as_ref(),
                                    strategy.name(),
                                )
                        })
                        .cloned()
                        .collect();
                    let tasks = strategies
                        .into_iter()
                        .map(|strategy| self.scheduled_task(strategy, timeout, None, None))
                        .collect::<Vec<_>>();
                    if tasks.is_empty() {
                        continue;
                    }
                    return Ok(tasks);
                }
                SchedulerPhase::Annealing => {
                    self.baseline_elapsed
                        .get_or_insert_with(|| self.started.elapsed());
                    self.sa_started.get_or_insert_with(Instant::now);
                    return self.next_annealing_task();
                }
                SchedulerPhase::Done => return Ok(Vec::new()),
            }
        }
    }

    fn observe(&mut self, result: &mut TaskResult) -> Result<(), SolverError> {
        let improved = result
            .candidate
            .metadata
            .improved_global_best
            .unwrap_or(false);
        if improved {
            self.best_solution = result.candidate.route.clone();
        }
        if result.candidate.strategy == "exact_bit_dp"
            && result.candidate.feasible
            && result.candidate.route.is_some()
            && !self.config.compare_all
        {
            self.phase = SchedulerPhase::Done;
            self.baseline_elapsed = Some(self.started.elapsed());
            self.termination_reason = Some("exact_optimum");
            return Ok(());
        }
        let Some(pending) = self
            .pending_sa
            .take()
            .filter(|pending| pending.task_id == result.task_id)
        else {
            return Ok(());
        };
        self.sa_run_count = self.sa_run_count.saturating_add(1);
        result
            .candidate
            .metadata
            .initial_strategy
            .get_or_insert_with(|| pending.initializer.label().to_owned());
        result.candidate.metadata.seed.get_or_insert(pending.seed);
        if let (Some(selector), Some(selection)) =
            (self.adaptive_policy.as_mut(), pending.arm_selection)
        {
            result.candidate.metadata.scheduler_reason = Some(selection.reason);
            result.candidate.metadata.scheduler_score =
                selection.score.map(|score| score.to_string());
            let eliminated = selector
                .observe(
                    &selection.arm_id,
                    if improved { 1.0 } else { 0.0 },
                    duration_ms(result.candidate.elapsed),
                    result.candidate.metadata.reserved_slot_ms.unwrap_or(0),
                    duration_ms(self.started.elapsed()),
                )
                .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))?;
            if !eliminated.is_empty() {
                result.candidate.metadata.scheduler_eliminations = Some(eliminated);
            }
        }
        if result.candidate.metadata.iteration_count == Some(0) {
            self.exhausted = true;
            self.phase = SchedulerPhase::Done;
            self.termination_reason = Some("exhausted");
        }
        Ok(())
    }

    fn cancel(&mut self) {
        self.phase = SchedulerPhase::Done;
        self.termination_reason = Some("cancellation");
    }
}

fn solver_enabled(config: &SolverOrchestratorConfig, solver_id: &str) -> bool {
    config.enabled_solver_ids.as_ref().is_none_or(|enabled| {
        enabled
            .iter()
            .any(|configured| configured.as_str() == solver_id)
    })
}

fn instance_policy_allows(selection: Option<&InstancePolicySelection>, strategy: &str) -> bool {
    let solver_id = solver_id_for_strategy(strategy);
    selection.is_none_or(|selection| selection.selected_solver_ids.contains(&solver_id))
}
