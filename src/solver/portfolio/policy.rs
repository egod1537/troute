use std::{
    collections::{BTreeMap, HashSet},
    fmt,
    str::FromStr,
    time::Duration,
};

use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    domain::{OptimizationProblem, StartPolicy},
    matrix::TravelTimeMatrix,
};

use super::{RunId, SolverDescriptor, SolverId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PortfolioPolicy {
    #[default]
    Current,
    ParallelAll,
    RoundRobin,
    WeightedFair,
    Racing,
    Ucb1,
    ThompsonSampling,
    InstanceAware,
}

impl PortfolioPolicy {
    pub const fn all() -> &'static [Self; 8] {
        &[
            Self::Current,
            Self::ParallelAll,
            Self::RoundRobin,
            Self::WeightedFair,
            Self::Racing,
            Self::Ucb1,
            Self::ThompsonSampling,
            Self::InstanceAware,
        ]
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Current => "CURRENT",
            Self::ParallelAll => "PARALLEL_ALL",
            Self::RoundRobin => "ROUND_ROBIN",
            Self::WeightedFair => "WEIGHTED_FAIR",
            Self::Racing => "RACING",
            Self::Ucb1 => "UCB1",
            Self::ThompsonSampling => "THOMPSON_SAMPLING",
            Self::InstanceAware => "INSTANCE_AWARE",
        }
    }
}

impl fmt::Display for PortfolioPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for PortfolioPolicy {
    type Err = PolicyParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::all()
            .iter()
            .copied()
            .find(|policy| policy.as_str().eq_ignore_ascii_case(value.trim()))
            .ok_or_else(|| PolicyParseError(value.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown portfolio policy: {0}")]
pub struct PolicyParseError(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortfolioPolicyConfig {
    pub policy: PortfolioPolicy,
    pub policy_version: String,
    pub seed: u64,
    pub min_quantum_ms: u64,
    pub racing_warmup_runs: usize,
    pub ucb_exploration: f64,
    pub min_exploration_ratio: f64,
    pub max_starvation_runs: u64,
    pub weights: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortfolioExperimentOptions {
    pub policy: PortfolioPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_budget_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_cpu_slots: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solver_ids: Option<Vec<SolverId>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detailed_trace: Option<bool>,
}

impl Default for PortfolioPolicyConfig {
    fn default() -> Self {
        Self {
            policy: PortfolioPolicy::Current,
            policy_version: "1".to_owned(),
            seed: 42,
            min_quantum_ms: 250,
            racing_warmup_runs: 1,
            ucb_exploration: 2.0_f64.sqrt(),
            min_exploration_ratio: 0.1,
            max_starvation_runs: 8,
            weights: BTreeMap::new(),
        }
    }
}

impl PortfolioPolicyConfig {
    pub fn validate(&self) -> Result<(), PolicyConfigError> {
        if self.min_quantum_ms == 0
            || self.racing_warmup_runs == 0
            || !self.ucb_exploration.is_finite()
            || self.ucb_exploration <= 0.0
            || !self.min_exploration_ratio.is_finite()
            || !(0.0..=1.0).contains(&self.min_exploration_ratio)
            || self.max_starvation_runs == 0
            || self
                .weights
                .values()
                .any(|weight| !weight.is_finite() || *weight <= 0.0)
        {
            return Err(PolicyConfigError::InvalidValue);
        }
        Ok(())
    }

    pub fn quantum(&self) -> Duration {
        Duration::from_millis(self.min_quantum_ms)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PolicyConfigError {
    #[error("portfolio policy values must be finite and positive and ratios must be in [0, 1]")]
    InvalidValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PolicyDescriptor {
    pub id: PortfolioPolicy,
    pub name: String,
    pub version: String,
    pub preemptive: bool,
    pub description: String,
}

pub fn policy_descriptors() -> Vec<PolicyDescriptor> {
    PortfolioPolicy::all()
        .iter()
        .map(|policy| PolicyDescriptor {
            id: *policy,
            name: policy.as_str().replace('_', " "),
            version: "1".to_owned(),
            preemptive: false,
            description: match policy {
                PortfolioPolicy::Current => "Legacy exact-first and chunked-SA behavior.",
                PortfolioPolicy::ParallelAll => {
                    "Launch every applicable solver within resource caps."
                }
                PortfolioPolicy::RoundRobin => {
                    "Rotate restartable arms; long-running solvers are non-preemptible."
                }
                PortfolioPolicy::WeightedFair => {
                    "Allocate restartable run slot-time by configured weights."
                }
                PortfolioPolicy::Racing => {
                    "Eliminate weak restartable arms after deterministic warm-up."
                }
                PortfolioPolicy::Ucb1 => {
                    "Select restartable arms with duration-aligned UCB1 rewards."
                }
                PortfolioPolicy::ThompsonSampling => {
                    "Sample Bernoulli improvement posteriors with a fixed seed."
                }
                PortfolioPolicy::InstanceAware => {
                    "Choose a deterministic rule from input-only instance features."
                }
            }
            .to_owned(),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SchedulingDecision {
    Launch {
        solver_id: SolverId,
        run_id: RunId,
        budget_ms: u64,
    },
    Queue {
        solver_id: SolverId,
        run_id: RunId,
        reason: String,
    },
    Cancel {
        run_id: RunId,
        reason: String,
    },
    Restart {
        solver_id: SolverId,
        previous_run_id: RunId,
        run_id: RunId,
    },
    StopExperiment {
        reason: String,
    },
    NoOp,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmState {
    pub solver_id: SolverId,
    pub run_count: u64,
    pub total_reserved_slot_ms: u64,
    pub total_observed_wall_ms: u64,
    pub improvement_count: u64,
    pub reward_sum: f64,
    pub reward_mean: f64,
    pub last_updated_ms: u64,
    pub available: bool,
}

impl ArmState {
    pub fn new(solver_id: SolverId) -> Self {
        Self {
            solver_id,
            run_count: 0,
            total_reserved_slot_ms: 0,
            total_observed_wall_ms: 0,
            improvement_count: 0,
            reward_sum: 0.0,
            reward_mean: 0.0,
            last_updated_ms: 0,
            available: true,
        }
    }

    pub fn observe(&mut self, reward: f64, wall_ms: u64, reserved_slot_ms: u64, now_ms: u64) {
        self.run_count += 1;
        self.total_observed_wall_ms = self.total_observed_wall_ms.saturating_add(wall_ms);
        self.total_reserved_slot_ms = self.total_reserved_slot_ms.saturating_add(reserved_slot_ms);
        self.reward_sum += reward.clamp(0.0, 1.0);
        self.reward_mean = self.reward_sum / self.run_count as f64;
        self.improvement_count += u64::from(reward > 0.0);
        self.last_updated_ms = now_ms;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmSelection {
    pub arm_id: String,
    pub score: Option<f64>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmElimination {
    pub arm_id: String,
    pub sample_count: u64,
    pub reason: String,
    pub estimated_budget_saved_ms: u64,
}

#[derive(Debug, Clone)]
struct SelectableArm {
    id: String,
    state: ArmState,
    eliminated: bool,
    last_pulled_at: u64,
}

/// Deterministic per-Job controller shared by every restartable policy.
/// A pull is counted only by `observe`, after the corresponding run returns.
#[derive(Debug, Clone)]
pub struct RestartableArmSelector {
    config: PortfolioPolicyConfig,
    arms: Vec<SelectableArm>,
    cursor: usize,
    total_completed: u64,
    racing_stage_target: u64,
    rng: StdRng,
}

impl RestartableArmSelector {
    pub fn new(
        config: PortfolioPolicyConfig,
        arm_ids: Vec<String>,
    ) -> Result<Self, ArmSelectionError> {
        config
            .validate()
            .map_err(|_| ArmSelectionError::InvalidConfiguration)?;
        let unique = arm_ids.iter().collect::<HashSet<_>>();
        if arm_ids.is_empty()
            || unique.len() != arm_ids.len()
            || arm_ids.iter().any(|id| id.trim().is_empty())
        {
            return Err(ArmSelectionError::InvalidArms);
        }
        let arms = arm_ids
            .into_iter()
            .map(|id| SelectableArm {
                state: ArmState::new(
                    SolverId::new("sa").expect("the static SA solver ID is valid"),
                ),
                id,
                eliminated: false,
                last_pulled_at: 0,
            })
            .collect();
        let racing_stage_target = config.racing_warmup_runs as u64;
        let rng = StdRng::seed_from_u64(config.seed);
        Ok(Self {
            config,
            arms,
            cursor: 0,
            total_completed: 0,
            racing_stage_target,
            rng,
        })
    }

    pub fn select(&mut self) -> Result<ArmSelection, ArmSelectionError> {
        if self.arms.iter().all(|arm| arm.eliminated) {
            return Err(ArmSelectionError::NoAvailableArm);
        }
        if let Some(index) = self
            .arms
            .iter()
            .position(|arm| !arm.eliminated && arm.state.run_count == 0)
        {
            return Ok(self.selection(index, None, "initial exploration"));
        }

        if let Some(index) = self.active_indices().find(|index| {
            self.total_completed
                .saturating_sub(self.arms[*index].last_pulled_at)
                >= self.config.max_starvation_runs
        }) {
            return Ok(self.selection(index, None, "maximum starvation interval"));
        }
        let active_count = self.active_indices().count().max(1) as f64;
        let exploration_floor = self.config.min_exploration_ratio / active_count;
        if let Some(index) = self
            .active_indices()
            .min_by_key(|index| (self.arms[*index].state.run_count, *index))
        {
            let share =
                self.arms[index].state.run_count as f64 / self.total_completed.max(1) as f64;
            if share < exploration_floor {
                return Ok(self.selection(index, Some(share), "minimum exploration share"));
            }
        }

        let index = match self.config.policy {
            PortfolioPolicy::Current
            | PortfolioPolicy::ParallelAll
            | PortfolioPolicy::RoundRobin => self.next_round_robin(),
            PortfolioPolicy::WeightedFair | PortfolioPolicy::InstanceAware => self
                .active_indices()
                .min_by(|left, right| {
                    self.weighted_virtual_time(*left)
                        .total_cmp(&self.weighted_virtual_time(*right))
                        .then_with(|| left.cmp(right))
                })
                .ok_or(ArmSelectionError::NoAvailableArm)?,
            PortfolioPolicy::Racing => self
                .active_indices()
                .min_by_key(|index| (self.arms[*index].state.run_count, *index))
                .ok_or(ArmSelectionError::NoAvailableArm)?,
            PortfolioPolicy::Ucb1 => {
                let total = self.total_completed.max(1) as f64;
                self.active_indices()
                    .max_by(|left, right| {
                        self.ucb_score(*left, total)
                            .total_cmp(&self.ucb_score(*right, total))
                            .then_with(|| right.cmp(left))
                    })
                    .ok_or(ArmSelectionError::NoAvailableArm)?
            }
            PortfolioPolicy::ThompsonSampling => {
                let active: Vec<_> = self.active_indices().collect();
                let mut best = None;
                for index in active {
                    let state = &self.arms[index].state;
                    let alpha = 1.0 + state.improvement_count as f64;
                    let beta = 1.0 + state.run_count.saturating_sub(state.improvement_count) as f64;
                    let sampled = sample_beta(alpha, beta, &mut self.rng);
                    if best.as_ref().is_none_or(|(_, current)| sampled > *current) {
                        best = Some((index, sampled));
                    }
                }
                let (index, sampled) = best.ok_or(ArmSelectionError::NoAvailableArm)?;
                return Ok(self.selection(index, Some(sampled), "Beta posterior sample"));
            }
        };
        let (score, reason) = match self.config.policy {
            PortfolioPolicy::WeightedFair | PortfolioPolicy::InstanceAware => (
                Some(self.weighted_virtual_time(index)),
                "lowest weighted virtual slot-time",
            ),
            PortfolioPolicy::Racing => (
                Some(self.arms[index].state.reward_mean),
                "active racing arm",
            ),
            PortfolioPolicy::Ucb1 => (
                Some(self.ucb_score(index, self.total_completed.max(1) as f64)),
                "highest UCB1 score",
            ),
            _ => (None, "round-robin rotation"),
        };
        Ok(self.selection(index, score, reason))
    }

    pub fn observe(
        &mut self,
        arm_id: &str,
        reward: f64,
        wall_ms: u64,
        reserved_slot_ms: u64,
        now_ms: u64,
    ) -> Result<Vec<ArmElimination>, ArmSelectionError> {
        let arm = self
            .arms
            .iter_mut()
            .find(|arm| arm.id == arm_id)
            .ok_or_else(|| ArmSelectionError::UnknownArm(arm_id.to_owned()))?;
        if arm.eliminated {
            return Err(ArmSelectionError::EliminatedArm(arm_id.to_owned()));
        }
        arm.state.observe(reward, wall_ms, reserved_slot_ms, now_ms);
        self.total_completed += 1;
        arm.last_pulled_at = self.total_completed;
        let eliminated = if self.config.policy == PortfolioPolicy::Racing {
            self.advance_racing_stage()
        } else {
            Vec::new()
        };
        Ok(eliminated)
    }

    pub fn states(&self) -> Vec<(&str, &ArmState, bool)> {
        self.arms
            .iter()
            .map(|arm| (arm.id.as_str(), &arm.state, arm.eliminated))
            .collect()
    }

    fn active_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.arms
            .iter()
            .enumerate()
            .filter_map(|(index, arm)| (!arm.eliminated).then_some(index))
    }

    fn next_round_robin(&mut self) -> usize {
        loop {
            let index = self.cursor % self.arms.len();
            self.cursor = self.cursor.wrapping_add(1);
            if !self.arms[index].eliminated {
                return index;
            }
        }
    }

    fn configured_weight(&self, index: usize) -> f64 {
        let id = &self.arms[index].id;
        self.config
            .weights
            .get(id)
            .or_else(|| self.config.weights.get(&format!("sa:{id}")))
            .copied()
            .unwrap_or(1.0)
    }

    fn weighted_virtual_time(&self, index: usize) -> f64 {
        self.arms[index].state.total_reserved_slot_ms as f64 / self.configured_weight(index)
    }

    fn ucb_score(&self, index: usize, total: f64) -> f64 {
        let state = &self.arms[index].state;
        if state.run_count == 0 {
            return f64::INFINITY;
        }
        state.reward_mean
            + self.config.ucb_exploration * (total.ln() / state.run_count as f64).sqrt()
    }

    fn selection(&self, index: usize, score: Option<f64>, reason: &str) -> ArmSelection {
        ArmSelection {
            arm_id: self.arms[index].id.clone(),
            score,
            reason: reason.to_owned(),
        }
    }

    fn advance_racing_stage(&mut self) -> Vec<ArmElimination> {
        let active: Vec<_> = self.active_indices().collect();
        if active.len() <= 1
            || active
                .iter()
                .any(|index| self.arms[*index].state.run_count < self.racing_stage_target)
        {
            return Vec::new();
        }
        let mut ranked = active;
        ranked.sort_by(|left, right| {
            self.arms[*right]
                .state
                .reward_mean
                .total_cmp(&self.arms[*left].state.reward_mean)
                .then_with(|| left.cmp(right))
        });
        let keep = ranked.len().div_ceil(2);
        let mut eliminated = Vec::new();
        for index in ranked.into_iter().skip(keep) {
            self.arms[index].eliminated = true;
            self.arms[index].state.available = false;
            eliminated.push(ArmElimination {
                arm_id: self.arms[index].id.clone(),
                sample_count: self.arms[index].state.run_count,
                reason: "lower mean validated-improvement reward after racing stage".to_owned(),
                estimated_budget_saved_ms: self
                    .config
                    .min_quantum_ms
                    .saturating_mul(self.racing_stage_target),
            });
        }
        self.racing_stage_target = self.racing_stage_target.saturating_mul(2).max(1);
        eliminated
    }
}

fn sample_beta(alpha: f64, beta: f64, rng: &mut StdRng) -> f64 {
    let left = sample_gamma(alpha, rng);
    let right = sample_gamma(beta, rng);
    if left + right == 0.0 {
        0.5
    } else {
        left / (left + right)
    }
}

fn sample_gamma(shape: f64, rng: &mut StdRng) -> f64 {
    if shape < 1.0 {
        let uniform = rng.gen::<f64>().max(f64::MIN_POSITIVE);
        return sample_gamma(shape + 1.0, rng) * uniform.powf(1.0 / shape);
    }
    let d = shape - 1.0 / 3.0;
    let c = (9.0 * d).sqrt().recip();
    loop {
        let uniform_a = rng.gen::<f64>().max(f64::MIN_POSITIVE);
        let uniform_b = rng.gen::<f64>();
        let normal = (-2.0 * uniform_a.ln()).sqrt() * (std::f64::consts::TAU * uniform_b).cos();
        let factor = 1.0 + c * normal;
        if factor <= 0.0 {
            continue;
        }
        let value = factor * factor * factor;
        let acceptance = rng.gen::<f64>();
        if acceptance < 1.0 - 0.0331 * normal.powi(4)
            || acceptance.ln() < 0.5 * normal * normal + d * (1.0 - value + value.ln())
        {
            return d * value;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ArmSelectionError {
    #[error("restartable arms must be non-empty, unique, and named")]
    InvalidArms,
    #[error("portfolio policy configuration is invalid")]
    InvalidConfiguration,
    #[error("no restartable arm is available")]
    NoAvailableArm,
    #[error("unknown restartable arm: {0}")]
    UnknownArm(String),
    #[error("restartable arm was eliminated: {0}")]
    EliminatedArm(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceFeatures {
    pub location_count: usize,
    pub required_count: usize,
    pub optional_count: usize,
    pub stay_minutes_min: u32,
    pub stay_minutes_max: u32,
    pub stay_minutes_mean: f64,
    pub time_window_width_mean: f64,
    pub constrained_time_window_ratio: f64,
    pub narrow_time_window_ratio: f64,
    pub directed_asymmetry_ratio: f64,
    pub reachable_arc_ratio: f64,
    pub zero_minute_arc_ratio: f64,
    pub start_policy: StartPolicy,
}

impl InstanceFeatures {
    pub fn extract(problem: &OptimizationProblem, matrix: &TravelTimeMatrix) -> Self {
        let locations = problem.locations();
        let stays: Vec<_> = locations
            .iter()
            .map(|location| location.stay_minutes())
            .collect();
        let widths: Vec<_> = locations
            .iter()
            .map(|location| {
                u32::from(
                    location
                        .time_window()
                        .close()
                        .minutes()
                        .saturating_sub(location.time_window().open().minutes()),
                )
            })
            .collect();
        let mut arcs = 0_usize;
        let mut reachable = 0_usize;
        let mut zeros = 0_usize;
        let mut asymmetric = 0_usize;
        for from in 0..matrix.size() {
            for to in 0..matrix.size() {
                if from == to {
                    continue;
                }
                arcs += 1;
                if let Some(value) = matrix.travel_minutes(from, to) {
                    reachable += 1;
                    zeros += usize::from(value == 0);
                }
                if from < to && matrix.travel_minutes(from, to) != matrix.travel_minutes(to, from) {
                    asymmetric += 2;
                }
            }
        }
        let required_count = locations
            .iter()
            .filter(|location| location.required())
            .count();
        Self {
            location_count: locations.len(),
            required_count,
            optional_count: locations.len().saturating_sub(required_count),
            stay_minutes_min: stays.iter().copied().min().unwrap_or(0),
            stay_minutes_max: stays.iter().copied().max().unwrap_or(0),
            stay_minutes_mean: mean(&stays),
            time_window_width_mean: mean(&widths),
            constrained_time_window_ratio: ratio(
                widths.iter().filter(|width| **width < 23 * 60 + 50).count(),
                widths.len(),
            ),
            narrow_time_window_ratio: ratio(
                widths.iter().filter(|width| **width <= 120).count(),
                widths.len(),
            ),
            directed_asymmetry_ratio: ratio(asymmetric, arcs),
            reachable_arc_ratio: ratio(reachable, arcs),
            zero_minute_arc_ratio: ratio(zeros, arcs),
            start_policy: problem.start_policy(),
        }
    }
}

fn mean(values: &[u32]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().map(|value| f64::from(*value)).sum::<f64>() / values.len() as f64
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstancePolicySelection {
    pub rule_id: String,
    pub model_version: String,
    pub feature_schema_version: u16,
    pub selected_solver_ids: Vec<SolverId>,
    pub weights: BTreeMap<String, f64>,
    pub fallback_reasons: Vec<String>,
}

pub trait InstancePolicyModel: Send + Sync {
    fn model_version(&self) -> &str;
    fn training_dataset_id(&self) -> Option<&str>;
    fn select(
        &self,
        features: &InstanceFeatures,
        descriptors: &[SolverDescriptor],
    ) -> InstancePolicySelection;
}

#[derive(Debug, Default)]
pub struct RuleBasedInstancePolicy;

impl InstancePolicyModel for RuleBasedInstancePolicy {
    fn model_version(&self) -> &str {
        "rule-based-v1"
    }

    fn training_dataset_id(&self) -> Option<&str> {
        None
    }

    fn select(
        &self,
        features: &InstanceFeatures,
        descriptors: &[SolverDescriptor],
    ) -> InstancePolicySelection {
        let available = |id: &str| {
            descriptors.iter().any(|descriptor| {
                descriptor.id.as_str() == id
                    && descriptor.unavailable_reason.is_none()
                    && (features.optional_count == 0
                        || descriptor.capabilities.supports_optional_visit)
                    && (features.reachable_arc_ratio >= 1.0
                        || descriptor.capabilities.directed_sparse)
            })
        };
        let mut ids = Vec::new();
        let mut weights = BTreeMap::new();
        let mut fallback_reasons = Vec::new();
        let rule_id = if features.location_count <= 15 && available("exact_bit_dp") {
            ids.push(SolverId::new("exact_bit_dp").expect("static ID is valid"));
            if available("clustered") {
                ids.push(SolverId::new("clustered").expect("static ID is valid"));
            }
            if available("sa") {
                ids.push(SolverId::new("sa").expect("static ID is valid"));
            }
            weights.insert("exact_bit_dp".to_owned(), 4.0);
            weights.insert("clustered".to_owned(), 1.0);
            weights.insert("sa".to_owned(), 1.0);
            "small-exact-first"
        } else if features.reachable_arc_ratio < 0.8 {
            if available("mip_scip") {
                ids.push(SolverId::new("mip_scip").expect("static ID is valid"));
                weights.insert("mip_scip".to_owned(), 3.0);
            } else {
                fallback_reasons.push(
                    "mip_scip unavailable or inapplicable; using sparse-capable heuristics"
                        .to_owned(),
                );
            }
            for id in ["clustered", "partial_greedy_insertion", "sa"] {
                if available(id) {
                    ids.push(SolverId::new(id).expect("static ID is valid"));
                    weights.insert(id.to_owned(), 1.0);
                }
            }
            "sparse-mip-protected"
        } else {
            for (id, weight) in [
                ("greedy", 1.0),
                ("clustered", 2.0),
                (
                    "sa",
                    if features.location_count >= 30 {
                        4.0
                    } else {
                        2.0
                    },
                ),
            ] {
                if available(id) {
                    ids.push(SolverId::new(id).expect("static ID is valid"));
                    weights.insert(id.to_owned(), weight);
                }
            }
            "heuristic-scale-up"
        };
        InstancePolicySelection {
            rule_id: rule_id.to_owned(),
            model_version: self.model_version().to_owned(),
            feature_schema_version: 1,
            selected_solver_ids: ids,
            weights,
            fallback_reasons,
        }
    }
}
