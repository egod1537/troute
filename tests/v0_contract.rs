use troute::{
    domain::Location,
    matrix::TravelTimeMatrix,
    routing::{RoutingContext, RoutingError, RoutingProvider, TravelMode},
    solver::{RouteSolver, SolverError, SolverInput, SolverSolution},
    OptimizeRouteRequest, RouteOptimizationService,
};

struct IndexedRoutingProvider;

const INDEXED_TRAVEL_MINUTES: [[u32; 5]; 5] = [
    [0, 1, 10, 20, 30],
    [5, 0, 2, 40, 50],
    [6, 60, 0, 3, 70],
    [7, 80, 90, 0, 4],
    [8, 9, 10, 11, 0],
];

impl RoutingProvider for IndexedRoutingProvider {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        _context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let rows = INDEXED_TRAVEL_MINUTES[..locations.len()]
            .iter()
            .map(|row| row[..locations.len()].to_vec())
            .collect();
        TravelTimeMatrix::new(rows).map_err(|error| RoutingError::Provider(error.to_string()))
    }
}

struct InputOrderSolver;

impl RouteSolver for InputOrderSolver {
    fn solve(&self, input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        Ok(SolverSolution {
            visit_order: (0..input.problem.locations().len()).collect(),
        })
    }
}

struct FixedRoutingProvider;

impl RoutingProvider for FixedRoutingProvider {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
        _context: &RoutingContext,
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

struct ExpectedTravelModeProvider(TravelMode);

impl RoutingProvider for ExpectedTravelModeProvider {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        assert_eq!(context.travel_mode, self.0);
        let rows = INDEXED_TRAVEL_MINUTES[..locations.len()]
            .iter()
            .map(|row| row[..locations.len()].to_vec())
            .collect();
        TravelTimeMatrix::new(rows).map_err(RoutingError::InvalidMatrix)
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
                    "stay_minutes": 40
                },
                {
                    "id": "D",
                    "place_id": "google-place-id-d",
                    "open_time": "00:00",
                    "close_time": "23:50",
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
    assert_eq!(json["route"][1]["departure_time"], "11:40");
    assert_eq!(json["route"][2]["location_id"], "B");
    assert_eq!(json["route"][2]["arrival_time"], "12:05");
    assert_eq!(json["route"][2]["departure_time"], "13:35");
    assert_eq!(json["route"][3]["location_id"], "D");
    assert_eq!(json["route"][3]["arrival_time"], "14:05");
    assert_eq!(json["route"][3]["departure_time"], "14:05");
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

#[test]
fn travel_mode_defaults_to_transit_and_accepts_all_supported_values() {
    for (requested, expected) in [
        (None, TravelMode::Transit),
        (Some(TravelMode::Transit), TravelMode::Transit),
        (Some(TravelMode::Driving), TravelMode::Driving),
        (Some(TravelMode::Walking), TravelMode::Walking),
        (Some(TravelMode::Bicycling), TravelMode::Bicycling),
    ] {
        let mut request = debug_request(None);
        request.travel_mode = requested;
        RouteOptimizationService::new(ExpectedTravelModeProvider(expected), InputOrderSolver)
            .optimize(request)
            .unwrap();
    }
}

fn debug_request(debug: Option<serde_json::Value>) -> OptimizeRouteRequest {
    let mut value = serde_json::json!({
        "job_id": "route-debug-shuffle-test",
        "locations": [
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"B","place_id":"b","open_time":"00:00","close_time":"23:50","stay_minutes":20},
            {"id":"C","place_id":"c","open_time":"00:00","close_time":"23:50","stay_minutes":30},
            {"id":"D","place_id":"d","open_time":"00:00","close_time":"23:50","stay_minutes":40},
            {"id":"E","place_id":"e","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ],
        "start_time": "09:00"
    });
    if let Some(debug) = debug {
        value["debug"] = debug;
    }
    serde_json::from_value(value).unwrap()
}

fn optimize_debug(request: OptimizeRouteRequest) -> troute::OptimizeRouteResponse {
    RouteOptimizationService::new(IndexedRoutingProvider, InputOrderSolver)
        .optimize(request)
        .unwrap()
}

#[test]
fn omitted_or_false_debug_shuffle_preserves_the_solver_result() {
    let baseline = optimize_debug(debug_request(None));
    let explicitly_disabled = optimize_debug(debug_request(Some(serde_json::json!({
        "shuffle_result_route": false,
        "shuffle_seed": 1_234
    }))));

    assert_eq!(explicitly_disabled, baseline);
    assert_eq!(
        baseline
            .route
            .iter()
            .map(|stop| stop.location_id.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B", "C", "D", "E"]
    );
}

#[test]
fn seeded_debug_shuffle_rebuilds_the_entire_schedule_from_shuffled_legs() {
    let baseline = optimize_debug(debug_request(None));
    let request = || {
        debug_request(Some(serde_json::json!({
            "shuffle_result_route": true,
            "shuffle_seed": 1_234
        })))
    };
    let shuffled = optimize_debug(request());
    let repeated = optimize_debug(request());

    assert_eq!(shuffled, repeated);
    let ids = shuffled
        .route
        .iter()
        .map(|stop| stop.location_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids.first(), Some(&"A"));
    assert_eq!(ids.last(), Some(&"E"));
    assert_ne!(shuffled.route, baseline.route);
    let mut intermediate_ids = ids[1..ids.len() - 1].to_vec();
    intermediate_ids.sort_unstable();
    assert_eq!(intermediate_ids, vec!["B", "C", "D"]);

    let index_and_stay = |id: &str| match id {
        "A" => (0_usize, 0_u32),
        "B" => (1, 20),
        "C" => (2, 30),
        "D" => (3, 40),
        "E" => (4, 0),
        unexpected => panic!("unexpected location id: {unexpected}"),
    };
    assert_eq!(shuffled.route[0].arrival_time.minutes(), 9 * 60);
    assert_eq!(shuffled.route[0].departure_time.unwrap().minutes(), 9 * 60);

    let mut expected_total = 0_u32;
    let mut previous_departure = 9 * 60_u32;
    for edge in shuffled.route.windows(2) {
        let (from, _) = index_and_stay(&edge[0].location_id);
        let (to, stay) = index_and_stay(&edge[1].location_id);
        let travel = INDEXED_TRAVEL_MINUTES[from][to];
        expected_total += travel;
        let expected_arrival = previous_departure + travel;
        let expected_departure = expected_arrival + stay;
        assert_eq!(u32::from(edge[1].arrival_time.minutes()), expected_arrival);
        assert_eq!(
            u32::from(edge[1].departure_time.unwrap().minutes()),
            expected_departure
        );
        previous_departure = expected_departure;
    }
    assert_eq!(shuffled.total_travel_minutes, expected_total);
    assert_ne!(shuffled.total_travel_minutes, baseline.total_travel_minutes);
}

#[test]
fn debug_shuffle_leaves_a_two_location_route_unchanged() {
    let mut baseline_request = debug_request(None);
    baseline_request.locations = vec![
        baseline_request.locations[0].clone(),
        baseline_request.locations[4].clone(),
    ];
    let mut shuffled_request = baseline_request.clone();
    shuffled_request.debug = serde_json::from_value(serde_json::json!({
        "shuffle_result_route": true,
        "shuffle_seed": 1_234
    }))
    .unwrap();

    assert_eq!(
        optimize_debug(shuffled_request),
        optimize_debug(baseline_request)
    );
}
