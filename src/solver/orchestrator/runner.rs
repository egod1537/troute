use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    domain::TimeOfDay,
    solver::{
        DefaultObjectivePolicy, InstanceFeatures, InstancePolicyModel, InstancePolicySelection,
        ObjectiveEvaluator, PortfolioExperimentOptions, PortfolioPolicy, ResourceLimits,
        ResourceManager, RestartableArmSelector, RouteSolver, RuleBasedInstancePolicy,
        SolverDiagnostics, SolverError, SolverInput, SolverProgressEvent, SolverProgressObserver,
        SolverRegistry, SolverRunResult, SolverSolution, TelemetryMode,
    },
};

use super::{
    candidate_accumulator::CandidateAccumulator,
    diagnostics::{annealing_strategy, built_in_strategies, SaInitializer},
    initializer::{
        deterministic_sa_seed, InitializerSelectionPolicy, RoundRobinInitializerSelectionPolicy,
    },
    result::{assign_run_identity, OrchestratorSolveResult, SolveResultBuilder, SolveResultStats},
    strategy::{
        duration_ms, execute_strategy, report_progress, resource_request, solver_id_for_strategy,
        strategy_allowance, OrchestratedStrategy, StrategyBatchExecutor, StrategyTimeoutPolicy,
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
        let preview = super::candidate_preview::PreviewObserver::new(&self.evaluator, observer);
        let observer: &dyn SolverProgressObserver = &preview;
        let result = if self.anytime {
            self.solve_anytime(input, observer)
        } else {
            self.solve_registered(input, observer)
        };
        if result.is_ok() {
            preview.flush();
        }
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
            .enumerate()
            .filter(|(_, strategy)| strategy.is_applicable_to(input.problem))
            .map(|(index, strategy)| (index, Arc::clone(strategy)))
            .collect();
        if applicable.is_empty() {
            return Err(SolverError::NoFeasibleRoute);
        }

        let worker_count = self
            .config
            .max_concurrency
            .min(self.strategies.len())
            .max(1);
        let execution = StrategyBatchExecutor {
            input,
            observer,
            evaluator: &self.evaluator,
            resources: &self.resources,
            config: &self.config,
            started,
            timeout_policy: StrategyTimeoutPolicy::Fixed(self.config.strategy_timeout),
        };
        let indexed = execution.execute(applicable, worker_count);

        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let mut accumulator = CandidateAccumulator::with_capacity(indexed.len());
        for (_, mut candidate) in indexed {
            accumulator.consider(&self.evaluator, &mut candidate);
            accumulator.push(candidate);
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
        let deadline = started + self.config.total_budget;
        let mut accumulator = CandidateAccumulator::new();
        let instance_selection = (self.config.portfolio.policy == PortfolioPolicy::InstanceAware)
            .then(|| {
                RuleBasedInstancePolicy.select(
                    &InstanceFeatures::extract(input.problem, input.matrix),
                    self.registry.descriptors(),
                )
            });

        if let Some(exact) = self.strategies.iter().find(|strategy| {
            strategy.name() == "exact_bit_dp"
                && strategy.is_applicable_to(input.problem)
                && instance_policy_allows(instance_selection.as_ref(), strategy.name())
        }) {
            if let Some(timeout) = strategy_allowance(
                deadline,
                self.config.deadline_safety_margin,
                self.config.strategy_timeout,
            ) {
                report_progress(observer, || {
                    SolverProgressEvent::StrategyStarted(exact.name().to_owned())
                });
                let mut candidate = execute_strategy(
                    exact.as_ref(),
                    &self.evaluator,
                    SolverInput {
                        matrix: input.matrix,
                        problem: input.problem,
                        cancellation: input.cancellation,
                    },
                    timeout,
                    started,
                    &self.resources,
                    resource_request(&self.config, exact.name()),
                );
                report_progress(observer, || {
                    SolverProgressEvent::StrategyCompleted(exact.name().to_owned())
                });
                observer.on_candidate(&candidate);
                accumulator.consider(&self.evaluator, &mut candidate);
                let exact_optimum = candidate.feasible && candidate.route.is_some();
                accumulator.push(candidate);
                if exact_optimum && !self.config.compare_all {
                    let (candidates, global_best_updates) = accumulator.into_parts();
                    return SolveResultBuilder::new(&self.evaluator, observer).build(
                        candidates,
                        SolveResultStats {
                            total_budget: self.config.total_budget,
                            started,
                            baseline_elapsed: started.elapsed(),
                            sa_elapsed: Duration::ZERO,
                            sa_run_count: 0,
                            global_best_updates,
                            termination_reason: "exact_optimum",
                        },
                    );
                }
            }
        }

        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }

        let baseline_strategies: Vec<_> = self
            .strategies
            .iter()
            .filter(|strategy| {
                strategy.name() != "exact_bit_dp"
                    && strategy.is_applicable_to(input.problem)
                    && instance_policy_allows(instance_selection.as_ref(), strategy.name())
            })
            .cloned()
            .collect();
        let allowance = strategy_allowance(
            deadline,
            self.config.deadline_safety_margin,
            self.config.strategy_timeout,
        );
        if allowance.is_some() {
            let worker_count = self
                .config
                .max_concurrency
                .min(self.strategies.len())
                .max(1);
            let execution = StrategyBatchExecutor {
                input,
                observer,
                evaluator: &self.evaluator,
                resources: &self.resources,
                config: &self.config,
                started,
                timeout_policy: StrategyTimeoutPolicy::RemainingBudget {
                    deadline,
                    safety_margin: self.config.deadline_safety_margin,
                    cap: self.config.strategy_timeout,
                },
            };
            let baseline_results = execution.execute(
                baseline_strategies.into_iter().enumerate().collect(),
                worker_count,
            );
            for (_, mut candidate) in baseline_results {
                accumulator.consider(&self.evaluator, &mut candidate);
                accumulator.push(candidate);
            }
        }
        let baseline_elapsed = started.elapsed();
        let sa_started = Instant::now();
        let policy = RoundRobinInitializerSelectionPolicy;
        let mut adaptive_config = self.config.portfolio.clone();
        if adaptive_config.policy == PortfolioPolicy::InstanceAware
            && adaptive_config.weights.is_empty()
        {
            let features = InstanceFeatures::extract(input.problem, input.matrix);
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
        let mut adaptive_policy = (self.config.portfolio.policy != PortfolioPolicy::Current)
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
        let mut sa_run_count = 0_u64;
        let mut exhausted = false;

        while !input.cancellation.is_cancelled()
            && solver_enabled(&self.config, "sa")
            && instance_policy_allows(instance_selection.as_ref(), "sa")
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining <= self.config.deadline_safety_margin {
                break;
            }
            let usable = remaining - self.config.deadline_safety_margin;
            let policy_quantum = self.config.portfolio.quantum();
            if self.config.portfolio.policy != PortfolioPolicy::Current && usable < policy_quantum {
                break;
            }
            let requested_chunk = if self.config.portfolio.policy == PortfolioPolicy::Current {
                self.config.sa_chunk
            } else {
                self.config.sa_chunk.max(policy_quantum)
            };
            let chunk = requested_chunk.min(usable);
            if chunk.is_zero() {
                break;
            }
            let arm_selection = adaptive_policy
                .as_mut()
                .map(|selector| selector.select())
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
                None => policy.select(sa_run_count, accumulator.has_best()),
            };
            let warm_start = accumulator.warm_start();
            let round = sa_run_count / 4;
            let base_seed = self.config.sa_seeds[(round as usize) % self.config.sa_seeds.len()];
            let seed = deterministic_sa_seed(base_seed, initializer, round);
            let strategy = annealing_strategy(
                &self.config,
                initializer,
                warm_start,
                seed,
                sa_run_count + 1,
                duration_ms(chunk).saturating_sub(1).max(1),
            )?;
            report_progress(observer, || {
                SolverProgressEvent::AnnealingRunStarted(sa_run_count + 1)
            });
            report_progress(observer, || {
                SolverProgressEvent::StrategyStarted(strategy.name().to_owned())
            });
            let mut candidate = execute_strategy(
                strategy.as_ref(),
                &self.evaluator,
                SolverInput {
                    matrix: input.matrix,
                    problem: input.problem,
                    cancellation: input.cancellation,
                },
                chunk,
                started,
                &self.resources,
                resource_request(&self.config, strategy.name()),
            );
            report_progress(observer, || {
                SolverProgressEvent::StrategyCompleted(strategy.name().to_owned())
            });
            sa_run_count += 1;
            candidate
                .metadata
                .initial_strategy
                .get_or_insert_with(|| initializer.label().to_owned());
            candidate.metadata.seed.get_or_insert(seed);
            observer.on_candidate(&candidate);
            let improved = accumulator.consider(&self.evaluator, &mut candidate);
            if let Some(selection) = &arm_selection {
                candidate.metadata.scheduler_reason = Some(selection.reason.clone());
                candidate.metadata.scheduler_score = selection.score.map(|score| score.to_string());
            }
            if let Some(selector) = adaptive_policy.as_mut() {
                let eliminated = selector
                    .observe(
                        initializer.label(),
                        if improved { 1.0 } else { 0.0 },
                        duration_ms(candidate.elapsed),
                        candidate.metadata.reserved_slot_ms.unwrap_or(0),
                        duration_ms(started.elapsed()),
                    )
                    .map_err(|error| SolverError::InvalidConfiguration(error.to_string()))?;
                if !eliminated.is_empty() {
                    candidate.metadata.scheduler_eliminations = Some(eliminated);
                }
            }
            let no_iterations = candidate.metadata.iteration_count == Some(0);
            accumulator.push(candidate);
            if no_iterations {
                exhausted = true;
                break;
            }
        }

        let termination_reason = if input.cancellation.is_cancelled() {
            "cancellation"
        } else if exhausted {
            "exhausted"
        } else if Instant::now()
            >= deadline
                .checked_sub(self.config.deadline_safety_margin)
                .unwrap_or(deadline)
        {
            "deadline"
        } else {
            "exhausted"
        };
        let sa_elapsed = sa_started.elapsed();
        let (candidates, global_best_updates) = accumulator.into_parts();
        SolveResultBuilder::new(&self.evaluator, observer).build(
            candidates,
            SolveResultStats {
                total_budget: self.config.total_budget,
                started,
                baseline_elapsed,
                sa_elapsed,
                sa_run_count,
                global_best_updates,
                termination_reason,
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
