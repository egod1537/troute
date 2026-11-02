use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    domain::TimeOfDay,
    solver::{
        DefaultObjectivePolicy, ObjectiveEvaluator, PortfolioExperimentOptions, ResourceLimits,
        ResourceManager, RouteSolver, SolverDiagnostics, SolverError, SolverInput,
        SolverProgressEvent, SolverProgressObserver, SolverRegistry, SolverRunResult,
        SolverSolution, SolverTask, TaskId, TelemetryMode,
    },
};

use super::{
    candidate_accumulator::CandidateAccumulator,
    diagnostics::built_in_strategies,
    executor::TaskExecutor,
    result::{assign_run_identity, OrchestratorSolveResult, SolveResultBuilder, SolveResultStats},
    scheduler::{PortfolioTaskScheduler, ScheduledTask, TaskScheduler},
    strategy::{
        duration_ms, report_progress, resource_request, solver_id_for_strategy,
        OrchestratedStrategy,
    },
    telemetry::PortfolioTelemetryBuilder,
    CandidateSelector, SolverOrchestratorConfig,
};

pub struct SolverOrchestrator<E = DefaultObjectivePolicy> {
    config: SolverOrchestratorConfig,
    evaluator: E,
    strategies: Vec<Arc<dyn OrchestratedStrategy>>,
    anytime: bool,
    registry: SolverRegistry,
    resources: ResourceManager,
}

impl Default for SolverOrchestrator<DefaultObjectivePolicy> {
    fn default() -> Self {
        Self::new(SolverOrchestratorConfig::default())
            .expect("the default orchestrator configuration is valid")
    }
}

impl SolverOrchestrator<DefaultObjectivePolicy> {
    pub fn new(config: SolverOrchestratorConfig) -> Result<Self, SolverError> {
        let config = config.validate()?;
        let registry = SolverRegistry::built_in(&config)
            .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))?;
        let mut strategies = built_in_strategies(&config)?;
        strategies.retain(|strategy| {
            let id = solver_id_for_strategy(strategy.name());
            registry
                .descriptor(&id)
                .map_or(true, |descriptor| descriptor.unavailable_reason.is_none())
        });
        if let Some(enabled) = &config.enabled_solver_ids {
            strategies.retain(|strategy| {
                let id = solver_id_for_strategy(strategy.name());
                enabled.contains(&id)
            });
        }
        let resources = ResourceManager::new(ResourceLimits {
            logical_cpu_slots: config.max_concurrency,
            max_concurrent_runs: config.max_concurrency,
            max_external_processes: config.max_external_processes,
            memory_limit_bytes: config.memory_limit_bytes,
        })
        .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))?;
        Ok(Self {
            config,
            evaluator: DefaultObjectivePolicy,
            strategies,
            anytime: true,
            registry,
            resources,
        })
    }
}

impl<E: ObjectiveEvaluator> SolverOrchestrator<E> {
    /// Primarily useful for adding application-specific strategies and for
    /// testing failure/timeout isolation.
    pub fn with_strategies(
        config: SolverOrchestratorConfig,
        evaluator: E,
        strategies: Vec<Arc<dyn OrchestratedStrategy>>,
    ) -> Result<Self, SolverError> {
        let config = config.validate()?;
        validate_strategy_names(&strategies)?;
        let registry = SolverRegistry::built_in(&config)
            .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))?;
        let resources = ResourceManager::new(ResourceLimits {
            logical_cpu_slots: config.max_concurrency,
            max_concurrent_runs: config.max_concurrency,
            max_external_processes: config.max_external_processes,
            memory_limit_bytes: config.memory_limit_bytes,
        })
        .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))?;
        Ok(Self {
            config,
            evaluator,
            strategies,
            anytime: false,
            registry,
            resources,
        })
    }

    pub fn config(&self) -> &SolverOrchestratorConfig {
        &self.config
    }

    pub fn registry(&self) -> &SolverRegistry {
        &self.registry
    }

    pub fn solve_detailed(
        &self,
        input: SolverInput<'_>,
    ) -> Result<OrchestratorSolveResult, SolverError> {
        self.solve_detailed_with_progress(input, &crate::solver::NoopSolverProgressObserver)
    }
}

impl<E: ObjectiveEvaluator> RouteSolver for SolverOrchestrator<E> {
    fn portfolio_capabilities(&self) -> Option<crate::solver::PortfolioCapabilities> {
        Some(self.registry.capabilities(&self.config))
    }

    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        self.solve_detailed(input).map(|result| result.solution)
    }

    fn solve_with_diagnostics(
        &self,
        input: SolverInput<'_>,
    ) -> Result<SolverRunResult, SolverError> {
        self.solve_detailed(input).map(|result| SolverRunResult {
            solution: result.solution,
            diagnostics: Some(SolverDiagnostics {
                selected_strategy: result.selected_strategy,
                candidates: result.candidates,
                total_budget_ms: result.total_budget_ms,
                total_elapsed_ms: result.total_elapsed_ms,
                baseline_elapsed_ms: result.baseline_elapsed_ms,
                sa_elapsed_ms: result.sa_elapsed_ms,
                sa_run_count: result.sa_run_count,
                global_best_updates: result.global_best_updates,
                termination_reason: result.termination_reason,
                portfolio_telemetry: result.portfolio_telemetry,
            }),
        })
    }

    fn solve_with_diagnostics_and_progress(
        &self,
        input: SolverInput<'_>,
        observer: &dyn SolverProgressObserver,
    ) -> Result<SolverRunResult, SolverError> {
        self.solve_detailed_with_progress(input, observer)
            .map(|result| SolverRunResult {
                solution: result.solution,
                diagnostics: Some(SolverDiagnostics {
                    selected_strategy: result.selected_strategy,
                    candidates: result.candidates,
                    total_budget_ms: result.total_budget_ms,
                    total_elapsed_ms: result.total_elapsed_ms,
                    baseline_elapsed_ms: result.baseline_elapsed_ms,
                    sa_elapsed_ms: result.sa_elapsed_ms,
                    sa_run_count: result.sa_run_count,
                    global_best_updates: result.global_best_updates,
                    termination_reason: result.termination_reason,
                    portfolio_telemetry: result.portfolio_telemetry,
                }),
            })
    }

    fn solve_with_portfolio_options(
        &self,
        input: SolverInput<'_>,
        observer: &dyn SolverProgressObserver,
        options: Option<&PortfolioExperimentOptions>,
    ) -> Result<SolverRunResult, SolverError> {
        let Some(options) = options else {
            return self.solve_with_diagnostics_and_progress(input, observer);
        };
        if !self.config.experiment_override_enabled {
            return Err(SolverError::Unsupported(
                "per-request portfolio override requires TROUTE_PORTFOLIO_EXPERIMENT_ENABLED=true"
                    .to_owned(),
            ));
        }
        let mut config = self.config.clone();
        config.portfolio.policy = options.policy;
        if let Some(budget_ms) = options.total_budget_ms {
            if budget_ms == 0 || budget_ms > duration_ms(self.config.total_budget) {
                return Err(SolverError::InvalidConfiguration(format!(
                    "requested portfolio budget must be between 1 and {} ms",
                    duration_ms(self.config.total_budget)
                )));
            }
            config.total_budget = Duration::from_millis(budget_ms);
            config.strategy_timeout = config.strategy_timeout.min(config.total_budget);
            if config.deadline_safety_margin >= config.total_budget {
                config.deadline_safety_margin =
                    Duration::from_millis(1).min(config.total_budget / 2);
            }
        }
        if let Some(slots) = options.logical_cpu_slots {
            if slots == 0 || slots > self.config.max_concurrency {
                return Err(SolverError::InvalidConfiguration(format!(
                    "requested portfolio slots must be between 1 and {}",
                    self.config.max_concurrency
                )));
            }
            config.max_concurrency = slots;
        }
        if let Some(seed) = options.seed {
            config.portfolio.seed = seed;
            config.sa_seeds = vec![seed];
        }
        if let Some(ids) = &options.solver_ids {
            for id in ids {
                self.registry
                    .descriptor(id)
                    .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))?;
            }
            config.enabled_solver_ids = Some(ids.clone());
        }
        if options.detailed_trace == Some(true) {
            config.telemetry_mode = TelemetryMode::Detailed;
        }
        let mut solver = SolverOrchestrator::new(config)?;
        // Per-request policy/slot caps are local scheduling limits. Resource
        // admission remains shared with ordinary and other experimental Jobs
        // so an override cannot create an independent global CPU/process pool.
        solver.resources = self.resources.clone();
        solver.solve_with_diagnostics_and_progress(input, observer)
    }

    fn selected_start_time(
        &self,
        input: SolverInput<'_>,
        solution: &SolverSolution,
    ) -> Result<TimeOfDay, SolverError> {
        let metrics = self.evaluator.evaluate(input, solution)?;
        crate::solver::selected_start_time(input.problem, &metrics)
    }
}

impl<E: ObjectiveEvaluator> SolverOrchestrator<E> {
    fn solve_detailed_with_progress(
        &self,
        input: SolverInput<'_>,
        observer: &dyn SolverProgressObserver,
    ) -> Result<OrchestratorSolveResult, SolverError> {
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let location_count = input.problem.locations().len();
        if input.matrix.size() != location_count {
            return Err(SolverError::MatrixSizeMismatch {
                matrix: input.matrix.size(),
                locations: location_count,
            });
        }
        let result = if self.anytime {
            self.solve_anytime(input, observer)
        } else {
            self.solve_registered(input, observer)
        };
        result.map(|mut result| {
            result.portfolio_telemetry = Some(
                PortfolioTelemetryBuilder::new(&self.config, input.problem, input.matrix)
                    .build(&result),
            );
            result
        })
    }

    fn solve_registered(
        &self,
        input: SolverInput<'_>,
        observer: &dyn SolverProgressObserver,
    ) -> Result<OrchestratorSolveResult, SolverError> {
        let started = Instant::now();
        let applicable: Vec<_> = self
            .strategies
            .iter()
            .filter(|strategy| strategy.is_applicable_to(input.problem))
            .cloned()
            .enumerate()
            .map(|(index, strategy)| {
                let task = SolverTask::new(
                    TaskId::new((index + 1) as u64),
                    solver_id_for_strategy(strategy.name()),
                    self.config.strategy_timeout,
                    resource_request(&self.config, strategy.name()),
                );
                ScheduledTask::new(task, strategy)
            })
            .collect();
        if applicable.is_empty() {
            return Err(SolverError::NoFeasibleRoute);
        }

        let worker_count = self
            .config
            .max_concurrency
            .min(self.strategies.len())
            .max(1);
        let executor = TaskExecutor::new(&self.evaluator, &self.resources, observer, started, None);
        let results = executor.execute_batch(input, applicable, worker_count);

        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let mut accumulator = CandidateAccumulator::with_capacity(results.len());
        for mut result in results {
            accumulator.consider(&self.evaluator, &mut result.candidate);
            accumulator.push(result.candidate);
        }
        let (mut candidates, global_best_updates) = accumulator.into_parts();
        assign_run_identity(&mut candidates);
        report_progress(observer, || {
            SolverProgressEvent::SelectionStarted(candidates.len())
        });
        let selector = CandidateSelector::new(&self.evaluator);
        selector.rank(&mut candidates);
        let selected = selector.select(&candidates)?;
        let solution = selected.route.clone().ok_or(SolverError::NoFeasibleRoute)?;
        Ok(OrchestratorSolveResult {
            solution,
            selected_strategy: selected.strategy.clone(),
            candidates,
            total_budget_ms: duration_ms(self.config.total_budget),
            total_elapsed_ms: duration_ms(started.elapsed()),
            baseline_elapsed_ms: duration_ms(started.elapsed()),
            sa_elapsed_ms: 0,
            sa_run_count: 0,
            global_best_updates,
            termination_reason: "exhausted".to_owned(),
            portfolio_telemetry: None,
        })
    }

    fn solve_anytime(
        &self,
        input: SolverInput<'_>,
        observer: &dyn SolverProgressObserver,
    ) -> Result<OrchestratorSolveResult, SolverError> {
        let started = Instant::now();
        let mut scheduler = PortfolioTaskScheduler::new(
            &self.config,
            &self.strategies,
            &self.registry,
            input.problem,
            input.matrix,
            started,
        )?;
        let executor = TaskExecutor::new(
            &self.evaluator,
            &self.resources,
            observer,
            started,
            Some(scheduler.execution_deadline()),
        );
        let mut accumulator = CandidateAccumulator::new();

        loop {
            if input.cancellation.is_cancelled() {
                scheduler.cancel();
                break;
            }
            let mut tasks = scheduler.next_tasks()?;
            if tasks.is_empty() {
                break;
            }
            if tasks.len() == 1 && tasks[0].task.solver_id.as_str() == "sa" {
                let run = scheduler.summary().sa_run_count + 1;
                report_progress(observer, || SolverProgressEvent::AnnealingRunStarted(run));
            }
            let worker_count = scheduler.worker_count().min(tasks.len()).max(1);
            let results = if tasks.len() == 1 {
                vec![executor.execute(input, tasks.pop().expect("one task is present"))]
            } else {
                executor.execute_batch(input, tasks, worker_count)
            };
            for mut result in results {
                accumulator.consider(&self.evaluator, &mut result.candidate);
                scheduler.observe(&mut result)?;
                accumulator.push(result.candidate);
            }
        }

        if input.cancellation.is_cancelled() && !accumulator.has_best() {
            return Err(SolverError::Cancelled);
        }
        let summary = scheduler.summary();
        let (candidates, global_best_updates) = accumulator.into_parts();
        SolveResultBuilder::new(&self.evaluator, observer).build(
            candidates,
            SolveResultStats {
                total_budget: self.config.total_budget,
                started,
                baseline_elapsed: summary.baseline_elapsed,
                sa_elapsed: summary.sa_elapsed,
                sa_run_count: summary.sa_run_count,
                global_best_updates,
                termination_reason: &summary.termination_reason,
            },
        )
    }
}

fn validate_strategy_names(
    strategies: &[Arc<dyn OrchestratedStrategy>],
) -> Result<(), SolverError> {
    let mut names = HashSet::with_capacity(strategies.len());
    for strategy in strategies {
        if strategy.name().trim().is_empty() || !names.insert(strategy.name()) {
            return Err(SolverError::InvalidConfiguration(
                "orchestrated strategy names must be non-empty and unique".to_owned(),
            ));
        }
    }
    Ok(())
}
