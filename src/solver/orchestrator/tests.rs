use std::{
    sync::{
        atomic::{AtomicUsize, Ordering as TestOrdering},
        Arc,
    },
    thread,
    time::Duration,
};

use super::*;
use crate::{
    api::OptimizeRouteRequest,
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    solver::{
        DefaultObjectivePolicy, ObjectiveEvaluator, SimulatedAnnealingConfig, SolutionMetrics,
        SolverCandidate, SolverCandidateMetadata, SolverError, SolverInput, SolverSolution,
    },
};

#[derive(Clone)]
struct FixedStrategy {
    name: &'static str,
    route: Option<Vec<usize>>,
    fail: bool,
    wait_for_cancellation: bool,
}

struct ConcurrencyStrategy {
    name: String,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl OrchestratedStrategy for ConcurrencyStrategy {
    fn name(&self) -> &str {
        &self.name
    }

    fn execute(&self, _input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        let active = self.active.fetch_add(1, TestOrdering::SeqCst) + 1;
        self.peak.fetch_max(active, TestOrdering::SeqCst);
        thread::sleep(Duration::from_millis(15));
        self.active.fetch_sub(1, TestOrdering::SeqCst);
        Ok(StrategyExecution {
            solution: SolverSolution {
                visit_order: vec![0, 1],
            },
            metadata: SolverCandidateMetadata::default(),
        })
    }
}

impl OrchestratedStrategy for FixedStrategy {
    fn name(&self) -> &str {
        self.name
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        if self.wait_for_cancellation {
            while !input.cancellation.is_cancelled() {
                thread::sleep(Duration::from_millis(1));
            }
            return Err(SolverError::Cancelled);
        }
        if self.fail {
            return Err(SolverError::Failed("intentional failure".to_owned()));
        }
        Ok(StrategyExecution {
            solution: SolverSolution {
                visit_order: self.route.clone().unwrap(),
            },
            metadata: SolverCandidateMetadata::default(),
        })
    }
}

fn problem(location_count: usize) -> OptimizationProblem {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "orchestrator-test",
        "locations": (0..location_count).map(|index| serde_json::json!({
            "id": index.to_string(),
            "place_id": format!("place-{index}"),
            "open_time": "00:00",
            "close_time": "23:50",
            "stay_minutes": 0
        })).collect::<Vec<_>>(),
        "start_time": "09:00"
    }))
    .unwrap();
    request.try_into().unwrap()
}

fn matrix(size: usize) -> TravelTimeMatrix {
    TravelTimeMatrix::new(
        (0..size)
            .map(|from| {
                (0..size)
                    .map(|to| {
                        if from == to {
                            0
                        } else if to == from + 1 {
                            10
                        } else {
                            50
                        }
                    })
                    .collect()
            })
            .collect(),
    )
    .unwrap()
}

fn test_config() -> SolverOrchestratorConfig {
    SolverOrchestratorConfig {
        max_concurrency: 2,
        strategy_timeout: Duration::from_secs(2),
        total_budget: Duration::from_millis(75),
        sa_chunk: Duration::from_millis(10),
        deadline_safety_margin: Duration::from_millis(2),
        sa_config: SimulatedAnnealingConfig {
            iteration_limit: Some(50),
            seed: Some(42),
            ..SimulatedAnnealingConfig::default()
        },
        ..SolverOrchestratorConfig::default()
    }
}

fn input<'a>(
    problem: &'a OptimizationProblem,
    matrix: &'a TravelTimeMatrix,
    cancellation: &'a CancellationToken,
) -> SolverInput<'a> {
    SolverInput {
        matrix,
        problem,
        cancellation,
    }
}

include!("tests/selection.rs");
include!("tests/execution.rs");
include!("tests/portfolio.rs");
