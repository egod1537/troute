use std::{collections::HashSet, fmt, path::Path};

use serde::{de, Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::{
    domain::{OptimizationProblem, VisitPolicy},
    matrix::TravelTimeMatrix,
    solver::{SolverOrchestratorConfig, EXACT_MAX_LOCATIONS},
};

use super::{policy_descriptors, PolicyDescriptor, PortfolioPolicy};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SolverId(String);

impl SolverId {
    pub fn new(value: impl Into<String>) -> Result<Self, RegistryError> {
        let value = value.into();
        if value.is_empty()
            || !value.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
            })
        {
            return Err(RegistryError::InvalidSolverId(value));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SolverId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<'de> Deserialize<'de> for SolverId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RunId(u64);

impl RunId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SolverFamily {
    Exact,
    Mip,
    External,
    Heuristic,
    Initial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionModel {
    InProcess,
    NativeLibrary,
    ExternalProcess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolverCapabilities {
    pub supports_all_required: bool,
    pub supports_optional_visit: bool,
    pub directed_sparse: bool,
    pub time_windows_direct: bool,
    pub returns_feasible_candidate: bool,
    pub proof_support: bool,
    pub emits_intermediate_incumbent: bool,
    pub emits_native_bound: bool,
    pub supports_pause_resume: bool,
    pub restartable: bool,
    pub external_process: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupportedSize {
    pub minimum: usize,
    pub maximum: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCostHint {
    pub logical_cpu_slots: usize,
    pub native_threads: usize,
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolverDescriptor {
    pub id: SolverId,
    pub name: String,
    pub family: SolverFamily,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub capabilities: SolverCapabilities,
    pub execution_model: ExecutionModel,
    pub supported_size: SupportedSize,
    pub configured_threads: usize,
    pub resource_cost_hint: ResourceCostHint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApplicabilityStatus {
    Applicable,
    Inapplicable,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolverApplicability {
    pub solver_id: SolverId,
    pub status: ApplicabilityStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl SolverApplicability {
    pub fn is_applicable(&self) -> bool {
        self.status == ApplicabilityStatus::Applicable
    }
}

#[derive(Debug, Clone)]
pub struct SolverRegistry {
    descriptors: Vec<SolverDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortfolioCapabilityLimits {
    pub max_total_budget_ms: u64,
    pub max_logical_cpu_slots: usize,
    pub max_trace_events: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortfolioCapabilities {
    pub schema_version: u16,
    pub default_policy: PortfolioPolicy,
    pub experiment_override_enabled: bool,
    pub solvers: Vec<SolverDescriptor>,
    pub policies: Vec<PolicyDescriptor>,
    pub limits: PortfolioCapabilityLimits,
}

impl SolverRegistry {
    pub fn new(descriptors: Vec<SolverDescriptor>) -> Result<Self, RegistryError> {
        let mut ids = HashSet::with_capacity(descriptors.len());
        for descriptor in &descriptors {
            if descriptor.configured_threads == 0
                || descriptor.resource_cost_hint.logical_cpu_slots == 0
                || descriptor.resource_cost_hint.native_threads == 0
            {
                return Err(RegistryError::InvalidDescriptor {
                    solver_id: descriptor.id.clone(),
                    reason: "thread and CPU slot hints must be positive".to_owned(),
                });
            }
            if !ids.insert(descriptor.id.clone()) {
                return Err(RegistryError::DuplicateSolverId(descriptor.id.clone()));
            }
        }
        Ok(Self { descriptors })
    }

    pub fn built_in(config: &SolverOrchestratorConfig) -> Result<Self, RegistryError> {
        let standard = |restartable| SolverCapabilities {
            supports_all_required: true,
            supports_optional_visit: false,
            directed_sparse: false,
            time_windows_direct: true,
            returns_feasible_candidate: true,
            proof_support: false,
            emits_intermediate_incumbent: false,
            emits_native_bound: false,
            supports_pause_resume: false,
            restartable,
            external_process: false,
        };
        let cost = |threads| ResourceCostHint {
            logical_cpu_slots: threads,
            native_threads: threads,
            memory_bytes: None,
        };
        let descriptor = |id: &str,
                          name: &str,
                          family,
                          capabilities,
                          execution_model,
                          maximum,
                          threads,
                          unavailable_reason| {
            Ok(SolverDescriptor {
                id: SolverId::new(id)?,
                name: name.to_owned(),
                family,
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                capabilities,
                execution_model,
                supported_size: SupportedSize {
                    minimum: 2,
                    maximum,
                },
                configured_threads: threads,
                resource_cost_hint: cost(threads),
                unavailable_reason,
            })
        };

        let mut exact = standard(false);
        exact.supports_optional_visit = true;
        exact.directed_sparse = true;
        exact.proof_support = true;
        let mut mip = exact.clone();
        mip.emits_native_bound = true;
        let mip_reason = if !config.enable_mip_scip {
            Some("disabled by SOLVER_ENABLE_MIP_SCIP".to_owned())
        } else if !cfg!(feature = "solver-mip-scip") {
            Some("build without solver-mip-scip feature".to_owned())
        } else {
            None
        };
        let mut lkh = standard(true);
        lkh.time_windows_direct = false;
        lkh.external_process = true;
        let lkh_reason = if !config.enable_lkh3 {
            Some("disabled by SOLVER_ENABLE_LKH3".to_owned())
        } else if config
            .lkh3_executable
            .as_deref()
            .is_none_or(|path| !Path::new(path).is_file())
        {
            Some("LKH3_EXECUTABLE is missing or is not a file".to_owned())
        } else {
            None
        };
        let mut clustered = standard(false);
        clustered.supports_optional_visit = true;
        clustered.directed_sparse = true;
        let mut partial = standard(false);
        partial.supports_optional_visit = true;
        partial.directed_sparse = true;
        let mut sa = standard(true);
        sa.supports_optional_visit = true;
        sa.directed_sparse = true;

        Self::new(vec![
            descriptor(
                "exact_bit_dp",
                "Exact Bit DP",
                SolverFamily::Exact,
                exact,
                ExecutionModel::InProcess,
                Some(config.exact_limit.min(EXACT_MAX_LOCATIONS)),
                1,
                None,
            )?,
            descriptor(
                "mip_scip",
                "SCIP MIP",
                SolverFamily::Mip,
                mip,
                ExecutionModel::NativeLibrary,
                Some(100),
                config.mip_scip_threads,
                mip_reason,
            )?,
            descriptor(
                "lkh3",
                "LKH-3 ATSP Candidate",
                SolverFamily::External,
                lkh,
                ExecutionModel::ExternalProcess,
                None,
                1,
                lkh_reason,
            )?,
            descriptor(
                "clustered",
                "Clustered Exact",
                SolverFamily::Heuristic,
                clustered,
                ExecutionModel::InProcess,
                None,
                1,
                None,
            )?,
            descriptor(
                "partial_greedy_insertion",
                "Partial Greedy Insertion",
                SolverFamily::Heuristic,
                partial,
                ExecutionModel::InProcess,
                None,
                1,
                None,
            )?,
            descriptor(
                "greedy",
                "Greedy",
                SolverFamily::Initial,
                standard(false),
                ExecutionModel::InProcess,
                None,
                1,
                None,
            )?,
            descriptor(
                "mst_double_tree",
                "MST Double Tree",
                SolverFamily::Initial,
                standard(false),
                ExecutionModel::InProcess,
                None,
                1,
                None,
            )?,
            descriptor(
                "christofides",
                "Christofides",
                SolverFamily::Initial,
                standard(false),
                ExecutionModel::InProcess,
                None,
                1,
                None,
            )?,
            descriptor(
                "sa",
                "Simulated Annealing",
                SolverFamily::Heuristic,
                sa,
                ExecutionModel::InProcess,
                None,
                1,
                None,
            )?,
        ])
    }

    pub fn descriptors(&self) -> &[SolverDescriptor] {
        &self.descriptors
    }

    pub fn descriptor(&self, id: &SolverId) -> Result<&SolverDescriptor, RegistryError> {
        self.descriptors
            .iter()
            .find(|descriptor| &descriptor.id == id)
            .ok_or_else(|| RegistryError::UnknownSolverId(id.clone()))
    }

    pub fn applicability(
        &self,
        id: &SolverId,
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
    ) -> Result<SolverApplicability, RegistryError> {
        let descriptor = self.descriptor(id)?;
        let unavailable = |reason: String| SolverApplicability {
            solver_id: id.clone(),
            status: ApplicabilityStatus::Unavailable,
            reason: Some(reason),
        };
        if let Some(reason) = &descriptor.unavailable_reason {
            return Ok(unavailable(reason.clone()));
        }
        let n = problem.locations().len();
        if n < descriptor.supported_size.minimum
            || descriptor
                .supported_size
                .maximum
                .is_some_and(|maximum| n > maximum)
        {
            return Ok(SolverApplicability {
                solver_id: id.clone(),
                status: ApplicabilityStatus::Inapplicable,
                reason: Some(format!("location count {n} is outside the supported range")),
            });
        }
        if problem.visit_policy() == VisitPolicy::BestEffortOnFailure
            && !descriptor.capabilities.supports_optional_visit
        {
            return Ok(SolverApplicability {
                solver_id: id.clone(),
                status: ApplicabilityStatus::Inapplicable,
                reason: Some("optional visits are not supported".to_owned()),
            });
        }
        let sparse = (0..matrix.size()).any(|from| {
            (0..matrix.size()).any(|to| from != to && matrix.travel_minutes(from, to).is_none())
        });
        if sparse && !descriptor.capabilities.directed_sparse {
            return Ok(SolverApplicability {
                solver_id: id.clone(),
                status: ApplicabilityStatus::Inapplicable,
                reason: Some("directed sparse matrices are not supported".to_owned()),
            });
        }
        Ok(SolverApplicability {
            solver_id: id.clone(),
            status: ApplicabilityStatus::Applicable,
            reason: None,
        })
    }

    pub fn all_applicability(
        &self,
        problem: &OptimizationProblem,
        matrix: &TravelTimeMatrix,
    ) -> Vec<SolverApplicability> {
        self.descriptors
            .iter()
            .map(|descriptor| {
                self.applicability(&descriptor.id, problem, matrix)
                    .expect("registered solver IDs remain available")
            })
            .collect()
    }

    pub fn capabilities(&self, config: &SolverOrchestratorConfig) -> PortfolioCapabilities {
        PortfolioCapabilities {
            schema_version: 1,
            default_policy: config.portfolio.policy,
            experiment_override_enabled: config.experiment_override_enabled,
            solvers: self.descriptors.clone(),
            policies: policy_descriptors(),
            limits: PortfolioCapabilityLimits {
                max_total_budget_ms: u64::try_from(config.total_budget.as_millis())
                    .unwrap_or(u64::MAX),
                max_logical_cpu_slots: config.max_concurrency,
                max_trace_events: config.telemetry_max_events,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistryError {
    #[error("invalid solver ID: {0}")]
    InvalidSolverId(String),
    #[error("duplicate solver ID: {0}")]
    DuplicateSolverId(SolverId),
    #[error("unknown solver ID: {0}")]
    UnknownSolverId(SolverId),
    #[error("invalid descriptor for {solver_id}: {reason}")]
    InvalidDescriptor { solver_id: SolverId, reason: String },
}
