use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use super::*;
use crate::{
    development::{DevelopmentRouteSolver, DevelopmentRoutingProvider},
    domain::Location,
    events::{OptimizationErrorCode, ProgressStage},
    matrix::{MatrixService, TravelTimeMatrix},
    routing::TravelMode,
    solver::{
        ExactBitDpSolver, SimulatedAnnealingConfig, SolverOrchestrator, SolverOrchestratorConfig,
        SolverSolution,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordedEvent {
    Progress(ProgressStage, u8),
    Error(OptimizationErrorCode),
    Result(Box<OptimizeRouteResponse>),
}

#[derive(Default)]
struct InMemoryReporter {
    events: Mutex<Vec<RecordedEvent>>,
    messages: Mutex<Vec<Option<String>>>,
}

impl InMemoryReporter {
    fn events(&self) -> Vec<RecordedEvent> {
        self.events.lock().unwrap().clone()
    }

    fn messages(&self) -> Vec<Option<String>> {
        self.messages.lock().unwrap().clone()
    }
}

impl OptimizationEventReporter for InMemoryReporter {
    fn progress(&self, stage: ProgressStage, progress: u8, message: Option<&str>) {
        self.events
            .lock()
            .unwrap()
            .push(RecordedEvent::Progress(stage, progress));
        self.messages
            .lock()
            .unwrap()
            .push(message.map(str::to_owned));
    }

    fn error(&self, code: OptimizationErrorCode, _message: &str, _detail: &str) {
        self.events.lock().unwrap().push(RecordedEvent::Error(code));
    }

    fn result(&self, response: &OptimizeRouteResponse) {
        self.events
            .lock()
            .unwrap()
            .push(RecordedEvent::Result(Box::new(response.clone())));
    }
}

struct FailingRoutingProvider;

impl RoutingProvider for FailingRoutingProvider {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
        _context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        Err(RoutingError::Provider("provider is offline".to_owned()))
    }
}

struct UnusedSolver;

impl RouteSolver for UnusedSolver {
    fn solve(&self, _input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        unreachable!("routing failure must stop before solving")
    }
}

struct FixedRoutingProvider;

impl RoutingProvider for FixedRoutingProvider {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
        _context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        TravelTimeMatrix::new(vec![vec![0]])
            .map_err(|error| RoutingError::Provider(error.to_string()))
    }
}

struct CapturingRoutingProvider {
    modes: Arc<Mutex<Vec<TravelMode>>>,
}

impl RoutingProvider for CapturingRoutingProvider {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        self.modes.lock().unwrap().push(context.travel_mode);
        TravelTimeMatrix::new(vec![vec![0, 15], vec![15, 0]]).map_err(RoutingError::InvalidMatrix)
    }
}

struct TimeDependentRoutingProvider {
    leg_departures: Arc<Mutex<Vec<chrono::DateTime<chrono::Utc>>>>,
}

impl RoutingProvider for TimeDependentRoutingProvider {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
        _context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        TravelTimeMatrix::new(vec![vec![0, 15], vec![15, 0]]).map_err(RoutingError::InvalidMatrix)
    }

    fn travel_time_for_leg_with_control(
        &self,
        _from: &Location,
        _to: &Location,
        context: &RoutingContext,
        _cancellation: &CancellationToken,
        _deadline: Option<Instant>,
    ) -> Result<Option<u32>, RoutingError> {
        self.leg_departures
            .lock()
            .unwrap()
            .push(context.departure_time.unwrap());
        Ok(Some(25))
    }
}

struct NoFeasibleSolver;

impl RouteSolver for NoFeasibleSolver {
    fn solve(&self, _input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        Err(SolverError::NoFeasibleRoute)
    }
}

struct AlternativeModeRoutingProvider {
    modes: Arc<Mutex<Vec<TravelMode>>>,
}

impl RoutingProvider for AlternativeModeRoutingProvider {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        self.modes.lock().unwrap().push(context.travel_mode);
        if context.travel_mode == TravelMode::Walking {
            TravelTimeMatrix::new(vec![vec![0, 10], vec![10, 0]])
                .map_err(RoutingError::InvalidMatrix)
        } else {
            Err(RoutingError::Provider(format!(
                "{} route unavailable",
                context.travel_mode.as_provider_value()
            )))
        }
    }
}

include!("tests/pipeline.rs");
include!("tests/routing.rs");
include!("tests/remediation.rs");

fn valid_request() -> OptimizeRouteRequest {
    serde_json::from_value(serde_json::json!({
        "job_id": "route-service-test",
        "locations": [
            {
                "id": "A",
                "place_id": "place-a",
                "open_time": "00:00",
                "close_time": "23:50",
                "stay_minutes": 0
            },
            {
                "id": "B",
                "place_id": "place-b",
                "open_time": "00:00",
                "close_time": "23:50",
                "stay_minutes": 0
            }
        ],
        "start_time": "09:00"
    }))
    .unwrap()
}
