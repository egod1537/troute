use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use super::{ObjectiveVector, PortfolioPolicy, RunId, SolverId};

pub const TELEMETRY_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TelemetryMode {
    Off,
    #[default]
    Summary,
    Detailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TelemetryEventType {
    RunQueued,
    RunStarted,
    FirstFeasible,
    IncumbentImproved,
    BoundUpdated,
    RunFinished,
    RunTimedOut,
    RunCancelled,
    RunFailed,
    GlobalBestUpdated,
    SchedulerDecision,
    ResourceAllocated,
    ResourceReleased,
    ArmEliminated,
    Progress,
}

impl TelemetryEventType {
    fn is_critical(self) -> bool {
        !matches!(
            self,
            Self::Progress | Self::BoundUpdated | Self::SchedulerDecision
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TelemetryEventPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<ObjectiveVector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_objective: Option<ObjectiveVector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_proof_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_hint_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserved_slot_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_threads: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_cpu_slots: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_bound: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relative_gap_ppm: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reward_ppm: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_arm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision_score: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_remaining_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_budget_saved_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetryEvent {
    pub schema_version: u16,
    pub job_id: String,
    pub event_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver_id: Option<SolverId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    pub scheduler_policy: PortfolioPolicy,
    pub event_type: TelemetryEventType,
    pub elapsed_from_job_start_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at_unix_ms: Option<u64>,
    pub payload: TelemetryEventPayload,
}

#[derive(Debug)]
pub struct TelemetryCollector {
    job_id: String,
    policy: PortfolioPolicy,
    mode: TelemetryMode,
    max_events: usize,
    started: Instant,
    next_sequence: AtomicU64,
    events: Mutex<VecDeque<TelemetryEvent>>,
}

/// Observer boundary used by solver adapters. Implementations may persist,
/// stream, aggregate, or discard events without changing solver code.
pub trait TelemetrySink: Send + Sync {
    fn emit(
        &self,
        solver_id: Option<&SolverId>,
        run_id: Option<RunId>,
        event_type: TelemetryEventType,
        payload: TelemetryEventPayload,
    ) -> Option<TelemetryEvent>;
}

impl TelemetrySink for TelemetryCollector {
    fn emit(
        &self,
        solver_id: Option<&SolverId>,
        run_id: Option<RunId>,
        event_type: TelemetryEventType,
        payload: TelemetryEventPayload,
    ) -> Option<TelemetryEvent> {
        self.record(solver_id, run_id, event_type, payload)
    }
}

impl TelemetryCollector {
    pub fn new(
        job_id: impl Into<String>,
        policy: PortfolioPolicy,
        mode: TelemetryMode,
        max_events: usize,
    ) -> Self {
        Self {
            job_id: job_id.into(),
            policy,
            mode,
            max_events: max_events.max(1),
            started: Instant::now(),
            next_sequence: AtomicU64::new(1),
            events: Mutex::new(VecDeque::new()),
        }
    }

    pub fn record(
        &self,
        solver_id: Option<&SolverId>,
        run_id: Option<RunId>,
        event_type: TelemetryEventType,
        payload: TelemetryEventPayload,
    ) -> Option<TelemetryEvent> {
        self.record_inner(solver_id, run_id, event_type, payload, None)
    }

    pub fn record_at_elapsed(
        &self,
        solver_id: Option<&SolverId>,
        run_id: Option<RunId>,
        event_type: TelemetryEventType,
        payload: TelemetryEventPayload,
        elapsed_from_job_start_ms: u64,
    ) -> Option<TelemetryEvent> {
        self.record_inner(
            solver_id,
            run_id,
            event_type,
            payload,
            Some(elapsed_from_job_start_ms),
        )
    }

    fn record_inner(
        &self,
        solver_id: Option<&SolverId>,
        run_id: Option<RunId>,
        event_type: TelemetryEventType,
        payload: TelemetryEventPayload,
        requested_elapsed_ms: Option<u64>,
    ) -> Option<TelemetryEvent> {
        if self.mode == TelemetryMode::Off
            || (self.mode == TelemetryMode::Summary
                && matches!(
                    event_type,
                    TelemetryEventType::Progress | TelemetryEventType::BoundUpdated
                ))
        {
            return None;
        }
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if events.len() >= self.max_events {
            let removable = events
                .iter()
                .position(|existing| !existing.event_type.is_critical());
            if let Some(index) = removable {
                events.remove(index);
            } else if !event_type.is_critical() {
                // Never evict a lifecycle or improvement event merely to
                // retain another sampled trace event.
                return None;
            }
            // If every retained event is critical, the hard sampling target
            // may be exceeded. This is intentional: dropping a terminal or
            // incumbent event would make the trace unverifiable. The number
            // of such events is bounded by actual run lifecycle transitions.
        }
        let measured_elapsed =
            u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let elapsed_from_job_start_ms = requested_elapsed_ms.unwrap_or(measured_elapsed).max(
            events
                .back()
                .map_or(0, |event| event.elapsed_from_job_start_ms),
        );
        // Sequence allocation happens while holding the same lock used for
        // append order. Concurrent emitters therefore cannot invert sequence
        // numbers in the retained stream.
        let event = TelemetryEvent {
            schema_version: TELEMETRY_SCHEMA_VERSION,
            job_id: self.job_id.clone(),
            event_sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
            solver_id: solver_id.cloned(),
            run_id,
            scheduler_policy: self.policy,
            event_type,
            elapsed_from_job_start_ms,
            recorded_at_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|duration| u64::try_from(duration.as_millis()).ok()),
            payload,
        };
        events.push_back(event.clone());
        Some(event)
    }

    pub fn events(&self) -> Vec<TelemetryEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    pub fn mode(&self) -> TelemetryMode {
        self.mode
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolverRunSummary {
    pub solver_id: SolverId,
    pub run_id: RunId,
    pub status: String,
    pub feasible: bool,
    pub selected: bool,
    pub wall_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_hint_bytes: Option<u64>,
    pub reserved_slot_ms: u64,
    pub configured_threads: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<ObjectiveVector>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proof_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortfolioTelemetrySummary {
    pub schema_version: u16,
    pub policy: PortfolioPolicy,
    pub policy_version: String,
    pub total_budget_ms: u64,
    pub logical_cpu_slots: usize,
    pub seed: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_rule_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_model_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_feature_schema_version: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_selected_solver_ids: Option<Vec<SolverId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_weight_ppm: Option<BTreeMap<String, u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_fallback_reasons: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance_features: Option<BTreeMap<String, String>>,
    pub trace_collected: bool,
    pub runs: Vec<SolverRunSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<TelemetryEvent>,
}
