use troute::{
    domain::Location,
    matrix::TravelTimeMatrix,
    routing::{RoutingError, RoutingProvider},
    solver::{RouteSolver, SolverError, SolverInput, SolverSolution},
    OptimizeRouteRequest, RouteOptimizationService,
};

struct FixedRoutingProvider;

impl RoutingProvider for FixedRoutingProvider {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
    ) -> Result<TravelTimeMatrix, RoutingError> {
        TravelTimeMatrix::new(vec![
            vec![0, 30, 25, 40],
            vec![20, 0, 30, 30],
            vec![30, 25, 0, 20],
            vec![40, 30, 20, 0],
        ])
        .map_err(|error| RoutingError::Provider(error.to_string()))
    }
}

struct FixedSolver;

impl RouteSolver for FixedSolver {
    fn solve(&self, _input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        Ok(SolverSolution {
            visit_order: vec![0, 2, 1, 3],
        })
    }
}

#[test]
fn v0_pipeline_returns_ids_schedule_and_travel_total() {
    let request: OptimizeRouteRequest = serde_json::from_str(
        r#"{
            "job_id": "route-v0-contract-test",
            "locations": [
                {
                    "id": "A",
                    "place_id": "google-place-id-a",
                    "open_time": "09:00",
                    "close_time": "18:00",
                    "stay_minutes": 60
                },
                {
                    "id": "B",
                    "place_id": "google-place-id-b",
                    "open_time": "10:00",
                    "close_time": "20:00",
                    "stay_minutes": 90
                },
                {
                    "id": "C",
                    "place_id": "google-place-id-c",
                    "open_time": "11:00",
                    "close_time": "19:00",
                    "stay_minutes": 45
                },
                {
                    "id": "D",
                    "place_id": "google-place-id-d",
                    "open_time": "00:00",
                    "close_time": "23:59",
                    "stay_minutes": 0
                }
            ],
            "start_time": "09:00"
        }"#,
    )
    .unwrap();

    let response = RouteOptimizationService::new(FixedRoutingProvider, FixedSolver)
        .optimize(request)
        .unwrap();
    let json = serde_json::to_value(response).unwrap();

    assert_eq!(json["total_travel_minutes"], 80);
    assert_eq!(json["route"][0]["location_id"], "A");
    assert_eq!(json["route"][0]["departure_time"], "09:00");
    assert_eq!(json["route"][1]["location_id"], "C");
    assert_eq!(json["route"][1]["arrival_time"], "09:25");
    assert_eq!(json["route"][1]["departure_time"], "11:45");
    assert_eq!(json["route"][2]["location_id"], "B");
    assert_eq!(json["route"][2]["arrival_time"], "12:10");
    assert_eq!(json["route"][2]["departure_time"], "13:40");
    assert_eq!(json["route"][3]["location_id"], "D");
    assert_eq!(json["route"][3]["arrival_time"], "14:10");
    assert_eq!(json["route"][3]["departure_time"], "14:10");
}

#[test]
fn malformed_hhmm_time_is_rejected() {
    let result = serde_json::from_str::<OptimizeRouteRequest>(
        r#"{
            "job_id": "route-invalid-time-test",
            "locations": [],
            "start_time": "9:00"
        }"#,
    );

    assert!(result.is_err());
}
