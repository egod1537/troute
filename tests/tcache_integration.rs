mod support;

use std::{env, time::Duration};

use support::place_fixtures::{
    build_optimize_request_from_fixture, load_place_fixture, PLACE_FIXTURE_NAMES,
};
use troute::{
    domain::OptimizationProblem,
    routing::RoutingProvider,
    tcache::{TcacheRoutingConfig, TcacheRoutingProvider},
};

#[test]
#[ignore = "requires a live tcache server and billable Google Routes access"]
fn tokyo_fixture_resolves_through_tcache() {
    let fixture_name = "tokyo_3_places";
    assert!(PLACE_FIXTURE_NAMES.contains(&fixture_name));
    let fixture = load_place_fixture(fixture_name).expect("fixture setup must be valid");
    let request = build_optimize_request_from_fixture(fixture_name, "real-place-fixture-test")
        .expect("fixture setup must build a request");
    let problem = OptimizationProblem::try_from(request).expect("fixture request must be valid");
    let base_url = env::var("TCACHE_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:3200".to_owned())
        .parse()
        .expect("TCACHE_BASE_URL must be a valid URL");
    let provider = TcacheRoutingProvider::new(TcacheRoutingConfig {
        base_url,
        poll_interval: Duration::from_millis(250),
        timeout: Duration::from_secs(120),
    })
    .expect("tcache provider must initialize");

    let matrix = provider
        .travel_time_matrix(problem.locations())
        .unwrap_or_else(|error| {
            let places = fixture
                .locations
                .iter()
                .map(|location| {
                    format!("{} ({}, {})", location.name, location.id, location.place_id)
                })
                .collect::<Vec<_>>()
                .join(", ");
            panic!(
                "fixture={fixture_name}; stage=tcache matrix resolution; places=[{places}]; error={error}"
            )
        });

    assert_eq!(matrix.size(), fixture.locations.len());
    for index in 0..matrix.size() {
        assert_eq!(matrix.travel_minutes(index, index), Some(0));
    }
}
