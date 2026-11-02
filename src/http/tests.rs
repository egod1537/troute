use super::*;
use crate::{
    domain::Location,
    matrix::TravelTimeMatrix,
    routing::{RoutingContext, RoutingError, RoutingProvider},
    solver::{RouteSolver, SolverError, SolverInput, SolverSolution},
};
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tower::ServiceExt;

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
        unreachable!("routing failure must stop the pipeline before solving")
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

struct NoFeasibleSolver;

impl RouteSolver for NoFeasibleSolver {
    fn solve(&self, _input: SolverInput<'_>) -> Result<SolverSolution, SolverError> {
        Err(SolverError::NoFeasibleRoute)
    }
}

#[tokio::test]
async fn routing_failures_have_a_stable_json_mapping() {
    let app = router(RouteOptimizationService::new(
        FailingRoutingProvider,
        UnusedSolver,
    ));
    let request = Request::builder()
        .method("POST")
        .uri("/optimize")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(valid_request()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["error"]["code"], "ROUTING_UNAVAILABLE");
    assert_eq!(
        json["error"]["message"],
        "Travel-time routing is temporarily unavailable."
    );
    assert_eq!(
        json["error"]["failure_detail"]["type"],
        "ROUTING_UNAVAILABLE"
    );
    assert_eq!(json["error"]["suggestions"][0]["type"], "RETRY_ROUTING");
}

#[tokio::test]
async fn no_feasible_solver_result_has_a_stable_json_mapping() {
    let app = router(RouteOptimizationService::new(
        FixedRoutingProvider,
        NoFeasibleSolver,
    ));
    let request = Request::builder()
        .method("POST")
        .uri("/optimize")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(valid_request()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json: serde_json::Value =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(json["error"]["code"], "NO_FEASIBLE_ROUTE");
    assert_eq!(json["error"]["message"], "No feasible route was found.");
    assert_eq!(json["error"]["failure_detail"]["type"], "NO_FEASIBLE_ROUTE");
    assert_eq!(json["error"]["suggestions"][0]["type"], "START_EARLIER");
    assert_eq!(json["error"]["suggestions"][1]["type"], "REDUCE_STAY_TIME");
    assert_eq!(json["error"]["suggestions"][1]["location_id"], "place-2");
    assert_eq!(json["error"]["suggestions"][1]["current_minutes"], 600);
    assert_eq!(
        json["error"]["suggestions"][1]["suggested_max_minutes"],
        540
    );
    assert_eq!(
        json["error"]["suggestions"][2]["type"],
        "CHANGE_START_POLICY"
    );
}

fn valid_request() -> &'static str {
    r#"{
            "job_id":"route-http-test",
            "locations":[
                {
                    "id":"place-1",
                    "place_id":"GOOGLE_PLACE_ID_1",
                    "open_time":"09:00",
                    "close_time":"18:00",
                    "stay_minutes":0
                },
                {
                    "id":"place-2",
                    "place_id":"GOOGLE_PLACE_ID_2",
                    "open_time":"09:00",
                    "close_time":"18:00",
                    "stay_minutes":600
                }
            ],
            "start_time":"09:00"
        }"#
}
