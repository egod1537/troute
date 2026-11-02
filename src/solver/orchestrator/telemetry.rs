use std::collections::{HashMap, HashSet};

use crate::{
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    solver::{
        DefaultObjectivePolicy, InstanceFeatures, InstancePolicyModel, ObjectiveEvaluator,
        PortfolioPolicy, PortfolioTelemetrySummary, RuleBasedInstancePolicy, RunId, SolverId,
        SolverOutcomeKind, SolverRegistry, SolverRunSummary, TelemetryCollector,
        TelemetryEventPayload, TelemetryEventType, TELEMETRY_SCHEMA_VERSION,
    },
};

use super::{
    result::OrchestratorSolveResult,
    strategy::{duration_ms, solver_id_for_strategy},
    SolverOrchestratorConfig,
};

/// Converts completed solver candidates into the stable portfolio telemetry
/// model without coupling event construction to the orchestration flow.
pub(super) struct PortfolioTelemetryBuilder<'a> {
    config: &'a SolverOrchestratorConfig,
    problem: &'a OptimizationProblem,
    matrix: &'a TravelTimeMatrix,
}

impl<'a> PortfolioTelemetryBuilder<'a> {
    pub(super) fn new(
        config: &'a SolverOrchestratorConfig,
        problem: &'a OptimizationProblem,
        matrix: &'a TravelTimeMatrix,
    ) -> Self {
        Self {
            config,
            problem,
            matrix,
        }
    }

    pub(super) fn build(&self, result: &OrchestratorSolveResult) -> PortfolioTelemetrySummary {
        let config = self.config;
        let problem = self.problem;
        let matrix = self.matrix;
        let collector = TelemetryCollector::new(
            problem
                .locations()
                .first()
                .map_or("solver-run", |location| location.id()),
            config.portfolio.policy,
            config.telemetry_mode,
            config.telemetry_max_events,
        );
        let mut event_drafts = Vec::new();
        let mut draft_order = 0_u64;
        let mut draft = |elapsed_ms: u64,
                         solver_id: Option<&SolverId>,
                         run_id: Option<RunId>,
                         event_type: TelemetryEventType,
                         payload: TelemetryEventPayload| {
            draft_order = draft_order.saturating_add(1);
            event_drafts.push((
                elapsed_ms,
                draft_order,
                solver_id.cloned(),
                run_id,
                event_type,
                payload,
            ));
        };
        let mut completion_order: Vec<_> = result
            .candidates
            .iter()
            .enumerate()
            .filter_map(|(index, candidate)| {
                candidate.objective_score.map(|objective| {
                    (
                        candidate
                            .metadata
                            .job_finished_at_ms
                            .unwrap_or_else(|| duration_ms(candidate.elapsed)),
                        candidate.metadata.run_id.unwrap_or((index + 1) as u64),
                        objective,
                    )
                })
            })
            .collect();
        completion_order.sort_by_key(|(finished_at_ms, run_id, _)| (*finished_at_ms, *run_id));
        let evaluator = DefaultObjectivePolicy;
        let mut chronological_best = None;
        let mut global_update_run_ids = HashSet::new();
        let mut global_update_previous = HashMap::new();
        for (_, run_id, objective) in completion_order {
            if chronological_best.is_none_or(|best| evaluator.compare(&objective, &best).is_lt()) {
                global_update_previous.insert(run_id, chronological_best.map(Into::into));
                chronological_best = Some(objective);
                global_update_run_ids.insert(run_id);
            }
        }
        let mut runs = Vec::with_capacity(result.candidates.len());
        for (index, candidate) in result.candidates.iter().enumerate() {
            let solver_id = solver_id_for_strategy(&candidate.strategy);
            let run_id = RunId::new(candidate.metadata.run_id.unwrap_or((index + 1) as u64));
            let configured_threads = candidate.metadata.thread_count.unwrap_or_else(|| {
                if solver_id.as_str() == "mip_scip" {
                    config.mip_scip_threads
                } else {
                    1
                }
            });
            let wall_ms = duration_ms(candidate.elapsed);
            let queued_at_ms = candidate.metadata.job_queued_at_ms.unwrap_or(0);
            let started_at_ms = candidate.metadata.job_started_at_ms.unwrap_or_else(|| {
                queued_at_ms.saturating_add(candidate.metadata.queued_ms.unwrap_or(0))
            });
            let finished_at_ms = candidate
                .metadata
                .job_finished_at_ms
                .unwrap_or_else(|| started_at_ms.saturating_add(wall_ms));
            let reserved_slot_ms = candidate
                .metadata
                .reserved_slot_ms
                .unwrap_or_else(|| wall_ms.saturating_mul(configured_threads as u64));
            let objective = candidate.objective_score.map(Into::into);
            let status = candidate
                .metadata
                .outcome
                .map(|outcome| format!("{outcome:?}").to_ascii_uppercase())
                .unwrap_or_else(|| {
                    if candidate.feasible {
                        "COMPLETED"
                    } else {
                        "FAILED"
                    }
                    .to_owned()
                });
            draft(
                queued_at_ms,
                Some(&solver_id),
                Some(run_id),
                TelemetryEventType::SchedulerDecision,
                TelemetryEventPayload {
                    reason: candidate.metadata.scheduler_reason.clone().or_else(|| {
                        Some(format!("scheduled by {} policy", config.portfolio.policy))
                    }),
                    selected_arm: candidate.metadata.initial_strategy.clone(),
                    decision_score: candidate.metadata.scheduler_score.clone(),
                    budget_remaining_ms: Some(
                        duration_ms(config.total_budget).saturating_sub(queued_at_ms),
                    ),
                    logical_cpu_slots: Some(configured_threads),
                    policy_version: Some(config.portfolio.policy_version.clone()),
                    state_fingerprint: Some(format!(
                        "{}:{}:{}:{}",
                        config.portfolio.policy,
                        solver_id,
                        run_id.get(),
                        queued_at_ms
                    )),
                    ..TelemetryEventPayload::default()
                },
            );
            draft(
                queued_at_ms,
                Some(&solver_id),
                Some(run_id),
                TelemetryEventType::RunQueued,
                TelemetryEventPayload {
                    reason: Some(format!("scheduled by {} policy", config.portfolio.policy)),
                    ..TelemetryEventPayload::default()
                },
            );
            draft(
                started_at_ms,
                Some(&solver_id),
                Some(run_id),
                TelemetryEventType::ResourceAllocated,
                TelemetryEventPayload {
                    logical_cpu_slots: Some(configured_threads),
                    configured_threads: Some(configured_threads),
                    memory_hint_bytes: candidate.metadata.memory_hint_bytes,
                    ..TelemetryEventPayload::default()
                },
            );
            draft(
                started_at_ms,
                Some(&solver_id),
                Some(run_id),
                TelemetryEventType::RunStarted,
                TelemetryEventPayload::default(),
            );
            if candidate.feasible {
                draft(
                    finished_at_ms,
                    Some(&solver_id),
                    Some(run_id),
                    TelemetryEventType::FirstFeasible,
                    TelemetryEventPayload {
                        objective,
                        reason: Some(
                            "solver exposes no intermediate callback; discovery time is bounded by run completion"
                                .to_owned(),
                        ),
                        route_fingerprint: candidate
                            .route
                            .as_ref()
                            .map(|route| crate::solver::route_fingerprint(&route.visit_order)),
                        source_proof_status: candidate
                            .metadata
                            .proof_status
                            .map(|proof| format!("{proof:?}").to_ascii_uppercase()),
                        ..TelemetryEventPayload::default()
                    },
                );
            }
            if candidate.metadata.best_bound.is_some()
                || candidate.metadata.relative_gap_ppm.is_some()
            {
                draft(
                    finished_at_ms,
                    Some(&solver_id),
                    Some(run_id),
                    TelemetryEventType::BoundUpdated,
                    TelemetryEventPayload {
                        reason: Some(
                            "phase-specific native bound; not a portfolio-wide optimality proof"
                                .to_owned(),
                        ),
                        phase: candidate.metadata.objective_phase.clone(),
                        native_bound: candidate.metadata.best_bound.map(|bound| bound.to_string()),
                        relative_gap_ppm: candidate.metadata.relative_gap_ppm,
                        ..TelemetryEventPayload::default()
                    },
                );
            }
            if global_update_run_ids.contains(&run_id.get()) {
                draft(
                    finished_at_ms,
                    Some(&solver_id),
                    Some(run_id),
                    TelemetryEventType::IncumbentImproved,
                    TelemetryEventPayload {
                        objective,
                        previous_objective: global_update_previous
                            .get(&run_id.get())
                            .copied()
                            .flatten(),
                        reward_ppm: Some(1_000_000),
                        route_fingerprint: candidate
                            .route
                            .as_ref()
                            .map(|route| crate::solver::route_fingerprint(&route.visit_order)),
                        source_proof_status: candidate
                            .metadata
                            .proof_status
                            .map(|proof| format!("{proof:?}").to_ascii_uppercase()),
                        ..TelemetryEventPayload::default()
                    },
                );
                draft(
                    finished_at_ms,
                    Some(&solver_id),
                    Some(run_id),
                    TelemetryEventType::GlobalBestUpdated,
                    TelemetryEventPayload {
                        objective,
                        previous_objective: global_update_previous
                            .get(&run_id.get())
                            .copied()
                            .flatten(),
                        route_fingerprint: candidate
                            .route
                            .as_ref()
                            .map(|route| crate::solver::route_fingerprint(&route.visit_order)),
                        source_proof_status: candidate
                            .metadata
                            .proof_status
                            .map(|proof| format!("{proof:?}").to_ascii_uppercase()),
                        ..TelemetryEventPayload::default()
                    },
                );
            }
            for elimination in candidate.metadata.scheduler_eliminations.iter().flatten() {
                draft(
                    finished_at_ms,
                    Some(&solver_id),
                    Some(run_id),
                    TelemetryEventType::ArmEliminated,
                    TelemetryEventPayload {
                        selected_arm: Some(elimination.arm_id.clone()),
                        reason: Some(elimination.reason.clone()),
                        sample_count: Some(elimination.sample_count),
                        estimated_budget_saved_ms: Some(elimination.estimated_budget_saved_ms),
                        ..TelemetryEventPayload::default()
                    },
                );
            }
            let terminal = match candidate.metadata.outcome {
                Some(SolverOutcomeKind::Timeout) => TelemetryEventType::RunTimedOut,
                Some(SolverOutcomeKind::Cancelled) => TelemetryEventType::RunCancelled,
                Some(SolverOutcomeKind::Failed | SolverOutcomeKind::Unsupported) => {
                    TelemetryEventType::RunFailed
                }
                _ => TelemetryEventType::RunFinished,
            };
            draft(
                finished_at_ms,
                Some(&solver_id),
                Some(run_id),
                terminal,
                TelemetryEventPayload {
                    status: Some(status.clone()),
                    reason: candidate.metadata.error.clone(),
                    objective,
                    wall_ms: Some(wall_ms),
                    cpu_ms: candidate.metadata.cpu_ms,
                    peak_rss_bytes: candidate.metadata.peak_rss_bytes,
                    memory_hint_bytes: candidate.metadata.memory_hint_bytes,
                    reserved_slot_ms: Some(reserved_slot_ms),
                    configured_threads: Some(configured_threads),
                    measured: Some(true),
                    metric_source: Some(
                        if candidate.metadata.cpu_ms.is_some()
                            || candidate.metadata.peak_rss_bytes.is_some()
                        {
                            "orchestrator_monotonic_wall_clock_and_solver_process_metrics"
                        } else {
                            "orchestrator_monotonic_wall_clock; cpu_and_rss_unsupported"
                        }
                        .to_owned(),
                    ),
                    ..TelemetryEventPayload::default()
                },
            );
            draft(
                finished_at_ms,
                Some(&solver_id),
                Some(run_id),
                TelemetryEventType::ResourceReleased,
                TelemetryEventPayload {
                    reserved_slot_ms: Some(reserved_slot_ms),
                    ..TelemetryEventPayload::default()
                },
            );
            runs.push(SolverRunSummary {
                solver_id,
                run_id,
                status,
                feasible: candidate.feasible,
                selected: candidate.strategy == result.selected_strategy,
                wall_ms,
                cpu_ms: candidate.metadata.cpu_ms,
                peak_rss_bytes: candidate.metadata.peak_rss_bytes,
                memory_hint_bytes: candidate.metadata.memory_hint_bytes,
                reserved_slot_ms,
                configured_threads,
                objective,
                proof_status: candidate
                    .metadata
                    .proof_status
                    .map(|proof| format!("{proof:?}").to_ascii_uppercase()),
                seed: candidate.metadata.seed,
            });
        }
        drop(draft);
        event_drafts.sort_by_key(|(elapsed_ms, order, ..)| (*elapsed_ms, *order));
        for (elapsed_ms, _, solver_id, run_id, event_type, payload) in event_drafts {
            collector.record_at_elapsed(
                solver_id.as_ref(),
                run_id,
                event_type,
                payload,
                elapsed_ms,
            );
        }
        runs.sort_by_key(|run| run.run_id.get());
        let (
            instance_rule_id,
            instance_model_version,
            instance_feature_schema_version,
            instance_selected_solver_ids,
            instance_weight_ppm,
            instance_fallback_reasons,
            instance_features,
        ) = if config.portfolio.policy == PortfolioPolicy::InstanceAware {
            let features = InstanceFeatures::extract(problem, matrix);
            let registry = SolverRegistry::built_in(config)
                .expect("validated orchestrator config produces a valid built-in registry");
            let selection = RuleBasedInstancePolicy.select(&features, registry.descriptors());
            let weights = selection
                .weights
                .iter()
                .map(|(id, weight)| (id.clone(), (*weight * 1_000_000.0).round().max(0.0) as u64))
                .collect();
            let values = [
                ("location_count", features.location_count.to_string()),
                ("required_count", features.required_count.to_string()),
                ("optional_count", features.optional_count.to_string()),
                (
                    "reachable_arc_ratio",
                    features.reachable_arc_ratio.to_string(),
                ),
                (
                    "directed_asymmetry_ratio",
                    features.directed_asymmetry_ratio.to_string(),
                ),
                (
                    "narrow_time_window_ratio",
                    features.narrow_time_window_ratio.to_string(),
                ),
                (
                    "constrained_time_window_ratio",
                    features.constrained_time_window_ratio.to_string(),
                ),
                ("stay_minutes_min", features.stay_minutes_min.to_string()),
                ("stay_minutes_max", features.stay_minutes_max.to_string()),
                ("stay_minutes_mean", features.stay_minutes_mean.to_string()),
                (
                    "zero_minute_arc_ratio",
                    features.zero_minute_arc_ratio.to_string(),
                ),
                (
                    "start_policy",
                    format!("{:?}", features.start_policy).to_ascii_uppercase(),
                ),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect();
            (
                Some(selection.rule_id),
                Some(selection.model_version),
                Some(selection.feature_schema_version),
                Some(selection.selected_solver_ids),
                Some(weights),
                Some(selection.fallback_reasons),
                Some(values),
            )
        } else {
            (None, None, None, None, None, None, None)
        };
        PortfolioTelemetrySummary {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            policy: config.portfolio.policy,
            policy_version: config.portfolio.policy_version.clone(),
            total_budget_ms: duration_ms(config.total_budget),
            logical_cpu_slots: config.max_concurrency,
            seed: config.portfolio.seed,
            instance_rule_id,
            instance_model_version,
            instance_feature_schema_version,
            instance_selected_solver_ids,
            instance_weight_ppm,
            instance_fallback_reasons,
            instance_features,
            trace_collected: config.telemetry_mode == crate::solver::TelemetryMode::Detailed,
            runs,
            events: collector.events(),
        }
    }
}
