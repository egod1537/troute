use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use troute::solver::{
    portfolio::{
        PortfolioPolicy, ResourceLimits, ResourceManager, ResourceRequest, RestartableArmSelector,
        RunId, SolverId, SolverRegistry, TelemetryCollector, TelemetryEventPayload,
        TelemetryEventType, TelemetryMode,
    },
    DefaultObjectivePolicy, NoopSolverProgressObserver, OrchestratedStrategy,
    PortfolioPolicyConfig, RouteSolver, ScheduledTask, SolverCandidateMetadata, SolverError,
    SolverInput, SolverOrchestrator, SolverOrchestratorConfig, SolverSolution, SolverTask,
    StrategyExecution, TaskExecutor, TaskId, TaskResult, TaskScheduler, TaskState,
};

#[derive(Clone)]
struct ContractTaskStrategy {
    name: &'static str,
    panic: bool,
}

impl OrchestratedStrategy for ContractTaskStrategy {
    fn name(&self) -> &str {
        self.name
    }

    fn execute(&self, _input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        assert!(!self.panic, "intentional task panic");
        Ok(StrategyExecution {
            solution: SolverSolution {
                visit_order: vec![0, 1],
            },
            metadata: SolverCandidateMetadata::default(),
        })
    }
}

struct TwoRunScheduler {
    strategy: Arc<dyn OrchestratedStrategy>,
    next_id: u64,
    waiting: bool,
}

impl TaskScheduler for TwoRunScheduler {
    fn next_tasks(&mut self) -> Result<Vec<ScheduledTask>, SolverError> {
        if self.waiting || self.next_id > 2 {
            return Ok(Vec::new());
        }
        let id = self.next_id;
        self.next_id += 1;
        self.waiting = true;
        Ok(vec![ScheduledTask::new(
            SolverTask::new(
                TaskId::new(id),
                SolverId::new("greedy").unwrap(),
                Duration::from_millis(20),
                ResourceRequest {
                    logical_cpu_slots: 1,
                    native_threads: 1,
                    external_processes: 0,
                    memory_hint_bytes: None,
                },
            ),
            Arc::clone(&self.strategy),
        )])
    }

    fn observe(&mut self, result: &mut TaskResult) -> Result<(), SolverError> {
        assert_eq!(result.task_id.get(), self.next_id - 1);
        self.waiting = false;
        Ok(())
    }
}
use troute::{
    api::OptimizeRouteRequest,
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    storage::{FileJobStore, JobStore},
};

#[test]
fn built_in_solver_ids_are_stable_and_unique() {
    let registry = SolverRegistry::built_in(&SolverOrchestratorConfig::default()).unwrap();
    let ids: Vec<_> = registry
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.id.as_str())
        .collect();

    assert_eq!(
        ids,
        [
            "exact_bit_dp",
            "mip_scip",
            "lkh3",
            "clustered",
            "partial_greedy_insertion",
            "greedy",
            "mst_double_tree",
            "christofides",
            "sa",
        ]
    );
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), ids.len());
}

#[test]
fn all_portfolio_policy_wire_names_are_supported() {
    let policies = PortfolioPolicy::all();
    assert_eq!(policies.len(), 8);
    assert_eq!(PortfolioPolicy::default(), PortfolioPolicy::Current);
    assert_eq!(
        policies
            .iter()
            .map(|policy| serde_json::to_value(policy).unwrap())
            .collect::<Vec<_>>(),
        [
            "CURRENT",
            "PARALLEL_ALL",
            "ROUND_ROBIN",
            "WEIGHTED_FAIR",
            "RACING",
            "UCB1",
            "THOMPSON_SAMPLING",
            "INSTANCE_AWARE",
        ]
    );
}

#[test]
fn resource_leases_reserve_native_threads_and_release_on_drop() {
    let manager = ResourceManager::new(ResourceLimits {
        logical_cpu_slots: 4,
        max_concurrent_runs: 4,
        max_external_processes: 1,
        memory_limit_bytes: Some(1_024),
    })
    .unwrap();

    let first = manager
        .try_acquire(ResourceRequest {
            logical_cpu_slots: 3,
            native_threads: 3,
            external_processes: 0,
            memory_hint_bytes: Some(512),
        })
        .unwrap();
    assert!(manager
        .try_acquire(ResourceRequest {
            logical_cpu_slots: 2,
            native_threads: 2,
            external_processes: 0,
            memory_hint_bytes: Some(128),
        })
        .is_err());

    drop(first);
    assert!(manager
        .try_acquire(ResourceRequest {
            logical_cpu_slots: 2,
            native_threads: 2,
            external_processes: 0,
            memory_hint_bytes: Some(128),
        })
        .is_ok());

    let panic_manager = manager.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _lease = panic_manager
            .try_acquire(ResourceRequest {
                logical_cpu_slots: 1,
                native_threads: 1,
                external_processes: 0,
                memory_hint_bytes: None,
            })
            .unwrap();
        panic!("simulated solver panic");
    }));
    assert_eq!(
        manager.usage().active_runs,
        0,
        "panic must not leak a resource lease"
    );
}

#[test]
fn scheduler_creates_independent_follow_up_tasks_for_the_same_solver() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "task-restart-contract",
        "locations": [
            {"id":"start","place_id":"s","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"end","place_id":"e","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ]
    }))
    .unwrap();
    let problem = OptimizationProblem::try_from(request).unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0, 1], vec![1, 0]]).unwrap();
    let cancellation = CancellationToken::new();
    let resources = ResourceManager::new(ResourceLimits {
        logical_cpu_slots: 1,
        max_concurrent_runs: 1,
        max_external_processes: 1,
        memory_limit_bytes: None,
    })
    .unwrap();
    let evaluator = DefaultObjectivePolicy;
    let observer = NoopSolverProgressObserver;
    let executor = TaskExecutor::new(&evaluator, &resources, &observer, Instant::now(), None);
    let mut scheduler = TwoRunScheduler {
        strategy: Arc::new(ContractTaskStrategy {
            name: "greedy",
            panic: false,
        }),
        next_id: 1,
        waiting: false,
    };
    let mut identities = Vec::new();
    loop {
        let mut tasks = scheduler.next_tasks().unwrap();
        if tasks.is_empty() {
            break;
        }
        let mut result = executor.execute(
            SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &cancellation,
            },
            tasks.pop().unwrap(),
        );
        identities.push((result.task_id, result.solver_id.clone(), result.state));
        scheduler.observe(&mut result).unwrap();
    }

    assert_eq!(identities[0].0, TaskId::new(1));
    assert_eq!(identities[1].0, TaskId::new(2));
    assert_eq!(identities[0].1, identities[1].1);
    assert!(identities
        .iter()
        .all(|(_, _, state)| *state == TaskState::Completed));
}

#[test]
fn task_executor_isolates_panics_and_releases_leases() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "task-failure-contract",
        "locations": [
            {"id":"start","place_id":"s","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"end","place_id":"e","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ]
    }))
    .unwrap();
    let problem = OptimizationProblem::try_from(request).unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0, 1], vec![1, 0]]).unwrap();
    let cancellation = CancellationToken::new();
    let resources = ResourceManager::new(ResourceLimits {
        logical_cpu_slots: 1,
        max_concurrent_runs: 1,
        max_external_processes: 1,
        memory_limit_bytes: None,
    })
    .unwrap();
    let request = ResourceRequest {
        logical_cpu_slots: 1,
        native_threads: 1,
        external_processes: 0,
        memory_hint_bytes: None,
    };
    let task = |id, name, panic| {
        ScheduledTask::new(
            SolverTask::new(
                TaskId::new(id),
                SolverId::new("greedy").unwrap(),
                Duration::from_millis(20),
                request,
            ),
            Arc::new(ContractTaskStrategy { name, panic }),
        )
    };
    let evaluator = DefaultObjectivePolicy;
    let observer = NoopSolverProgressObserver;
    let executor = TaskExecutor::new(&evaluator, &resources, &observer, Instant::now(), None);
    let results = executor.execute_batch(
        SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &cancellation,
        },
        vec![task(1, "panicking", true), task(2, "healthy", false)],
        1,
    );

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].task_id, TaskId::new(1));
    assert_eq!(results[0].state, TaskState::Failed);
    assert!(results[0]
        .candidate
        .metadata
        .error
        .as_deref()
        .unwrap()
        .contains("panicked"));
    assert_eq!(results[1].task_id, TaskId::new(2));
    assert_eq!(results[1].state, TaskState::Completed);
    assert_eq!(resources.usage().active_runs, 0);
    assert_eq!(resources.usage().logical_cpu_slots, 0);
}

#[test]
fn telemetry_sequence_is_monotonic_and_progress_is_bounded() {
    let collector = TelemetryCollector::new(
        "portfolio-contract",
        PortfolioPolicy::RoundRobin,
        TelemetryMode::Detailed,
        8,
    );
    let solver_id = SolverId::new("sa").unwrap();
    let run_id = RunId::new(1);

    collector.record(
        Some(&solver_id),
        Some(run_id),
        TelemetryEventType::RunStarted,
        TelemetryEventPayload::default(),
    );
    for iteration in 0..100_000 {
        collector.record(
            Some(&solver_id),
            Some(run_id),
            TelemetryEventType::Progress,
            TelemetryEventPayload {
                message: Some(format!("iteration {iteration}")),
                ..TelemetryEventPayload::default()
            },
        );
    }
    collector.record(
        Some(&solver_id),
        Some(run_id),
        TelemetryEventType::RunFinished,
        TelemetryEventPayload {
            wall_ms: Some(Duration::from_millis(5).as_millis() as u64),
            ..TelemetryEventPayload::default()
        },
    );

    let events = collector.events();
    assert!(events.len() <= 8);
    assert!(events.windows(2).all(|pair| {
        pair[0].event_sequence < pair[1].event_sequence
            && pair[0].elapsed_from_job_start_ms <= pair[1].elapsed_from_job_start_ms
    }));
    assert_eq!(
        events.first().unwrap().event_type,
        TelemetryEventType::RunStarted
    );
    assert_eq!(
        events.last().unwrap().event_type,
        TelemetryEventType::RunFinished
    );
}

#[test]
fn telemetry_keeps_critical_events_and_orders_explicit_monotonic_times() {
    let collector = TelemetryCollector::new(
        "critical-telemetry",
        PortfolioPolicy::ParallelAll,
        TelemetryMode::Detailed,
        2,
    );
    let solver_id = SolverId::new("greedy").unwrap();
    let run_id = RunId::new(1);
    for (event_type, elapsed) in [
        (TelemetryEventType::RunStarted, 10),
        (TelemetryEventType::FirstFeasible, 20),
        (TelemetryEventType::GlobalBestUpdated, 20),
        (TelemetryEventType::RunFinished, 30),
    ] {
        collector.record_at_elapsed(
            Some(&solver_id),
            Some(run_id),
            event_type,
            TelemetryEventPayload::default(),
            elapsed,
        );
    }
    assert_eq!(
        collector.record_at_elapsed(
            Some(&solver_id),
            Some(run_id),
            TelemetryEventType::Progress,
            TelemetryEventPayload::default(),
            5,
        ),
        None
    );
    let events = collector.events();
    assert_eq!(
        events.len(),
        4,
        "critical lifecycle events are never sampled out"
    );
    assert!(events.windows(2).all(|pair| {
        pair[0].event_sequence < pair[1].event_sequence
            && pair[0].elapsed_from_job_start_ms <= pair[1].elapsed_from_job_start_ms
    }));
}

#[test]
fn every_policy_runs_through_the_common_orchestrator_contract() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "all-policy-contract",
        "start_policy": "FIXED",
        "start_time": "09:00",
        "locations": [
            {"id":"start","place_id":"s","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"middle","place_id":"m","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"end","place_id":"e","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ]
    }))
    .unwrap();
    let problem = OptimizationProblem::try_from(request).unwrap();
    let matrix = TravelTimeMatrix::new(vec![vec![0, 1, 5], vec![1, 0, 1], vec![5, 1, 0]]).unwrap();

    for policy in PortfolioPolicy::all() {
        let solver = SolverOrchestrator::new(SolverOrchestratorConfig {
            total_budget: Duration::from_millis(30),
            strategy_timeout: Duration::from_millis(20),
            deadline_safety_margin: Duration::from_millis(2),
            sa_chunk: Duration::from_millis(5),
            portfolio: PortfolioPolicyConfig {
                policy: *policy,
                min_quantum_ms: 5,
                ..PortfolioPolicyConfig::default()
            },
            ..SolverOrchestratorConfig::default()
        })
        .unwrap();
        let result = solver
            .solve_with_diagnostics(SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            })
            .unwrap();
        let diagnostics = result.diagnostics.unwrap();
        let telemetry = diagnostics.portfolio_telemetry.unwrap();
        assert_eq!(telemetry.policy, *policy);
        if *policy == PortfolioPolicy::InstanceAware {
            let selected = telemetry.instance_selected_solver_ids.as_ref().unwrap();
            assert!(telemetry
                .runs
                .iter()
                .all(|run| selected.contains(&run.solver_id)));
            assert!(telemetry.instance_model_version.is_some());
            assert!(telemetry.instance_features.is_some());
        }
        for run in &telemetry.runs {
            let events: Vec<_> = telemetry
                .events
                .iter()
                .filter(|event| event.run_id == Some(run.run_id))
                .collect();
            assert_eq!(
                events
                    .iter()
                    .filter(|event| event.event_type == TelemetryEventType::RunStarted)
                    .count(),
                1
            );
            assert_eq!(
                events
                    .iter()
                    .filter(|event| matches!(
                        event.event_type,
                        TelemetryEventType::RunFinished
                            | TelemetryEventType::RunTimedOut
                            | TelemetryEventType::RunCancelled
                            | TelemetryEventType::RunFailed
                    ))
                    .count(),
                1
            );
        }
        assert_eq!(result.solution.visit_order, vec![0, 1, 2]);
    }
}

#[test]
fn every_policy_preserves_no_route_semantics_for_unreachable_required_places() {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "all-policy-infeasible",
        "start_policy": "FIXED",
        "start_time": "09:00",
        "locations": [
            {"id":"start","place_id":"s","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"required","place_id":"m","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"end","place_id":"e","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ]
    }))
    .unwrap();
    let problem = OptimizationProblem::try_from(request).unwrap();
    let matrix = TravelTimeMatrix::new_sparse(vec![
        vec![Some(0), None, Some(1)],
        vec![None, Some(0), None],
        vec![Some(1), None, Some(0)],
    ])
    .unwrap();

    for policy in PortfolioPolicy::all() {
        let solver = SolverOrchestrator::new(SolverOrchestratorConfig {
            total_budget: Duration::from_millis(20),
            strategy_timeout: Duration::from_millis(10),
            deadline_safety_margin: Duration::from_millis(1),
            sa_chunk: Duration::from_millis(3),
            portfolio: PortfolioPolicyConfig {
                policy: *policy,
                min_quantum_ms: 3,
                ..PortfolioPolicyConfig::default()
            },
            ..SolverOrchestratorConfig::default()
        })
        .unwrap();
        assert!(matches!(
            solver.solve(SolverInput {
                matrix: &matrix,
                problem: &problem,
                cancellation: &CancellationToken::new(),
            }),
            Err(troute::solver::SolverError::NoFeasibleRoute)
        ));
    }
}

#[test]
fn adaptive_policy_arm_selection_is_seeded_and_observation_driven() {
    let arms = ["greedy", "mst", "christofides", "clustered"]
        .map(str::to_owned)
        .to_vec();
    for policy in [
        PortfolioPolicy::RoundRobin,
        PortfolioPolicy::WeightedFair,
        PortfolioPolicy::Racing,
        PortfolioPolicy::Ucb1,
        PortfolioPolicy::ThompsonSampling,
    ] {
        let config = PortfolioPolicyConfig {
            policy,
            seed: 7,
            weights: [("greedy".to_owned(), 1.0), ("clustered".to_owned(), 3.0)]
                .into_iter()
                .collect(),
            ..PortfolioPolicyConfig::default()
        };
        let run = || {
            let mut selector = RestartableArmSelector::new(config.clone(), arms.clone()).unwrap();
            let mut choices = Vec::new();
            for index in 0..12 {
                let choice = selector.select().unwrap();
                choices.push(choice.arm_id.clone());
                let reward = f64::from(choice.arm_id == "clustered" && index >= 3);
                selector
                    .observe(&choice.arm_id, reward, 10, 10, index * 10)
                    .unwrap();
            }
            choices
        };
        assert_eq!(run(), run(), "{policy} must be reproducible");
    }
}

#[test]
fn telemetry_jsonl_supports_cursor_reads_and_empty_legacy_jobs() {
    let temporary = tempfile::tempdir().unwrap();
    let store = FileJobStore::new(temporary.path()).unwrap();
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "telemetry-persistence",
        "locations": [
            {"id":"start","place_id":"s","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"end","place_id":"e","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ]
    }))
    .unwrap();
    store.create_job(&request).unwrap();
    assert!(store
        .list_solver_events(&request.job_id, 0, 10)
        .unwrap()
        .is_empty());

    let collector = TelemetryCollector::new(
        &request.job_id,
        PortfolioPolicy::Current,
        TelemetryMode::Detailed,
        10,
    );
    let first = collector
        .record(
            Some(&SolverId::new("greedy").unwrap()),
            Some(RunId::new(1)),
            TelemetryEventType::RunStarted,
            TelemetryEventPayload::default(),
        )
        .unwrap();
    let second = collector
        .record(
            Some(&SolverId::new("greedy").unwrap()),
            Some(RunId::new(1)),
            TelemetryEventType::RunFinished,
            TelemetryEventPayload::default(),
        )
        .unwrap();
    store.append_solver_event(&request.job_id, &first).unwrap();
    store.append_solver_event(&request.job_id, &second).unwrap();

    let page = store
        .list_solver_events(&request.job_id, first.event_sequence, 10)
        .unwrap();
    assert_eq!(page, vec![second]);
}
