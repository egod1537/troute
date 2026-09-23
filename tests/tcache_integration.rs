mod support;

use std::{env, time::Duration};

use chrono::{Duration as ChronoDuration, Utc};
use reqwest::Url;
use support::place_fixtures::{
    build_optimize_request_from_fixture, load_place_fixture, PLACE_FIXTURE_NAMES,
};
use troute::{
    domain::OptimizationProblem,
    routing::{PairwiseMatrixRoutingProvider, RoutingContext, RoutingProvider, TravelMode},
    tcache::{TcacheRoutingConfig, TcacheTravelTimeProvider},
};

#[test]
#[ignore = "requires a live tcache server and billable Google Routes access"]
fn tokyo_fixture_builds_pairwise_matrix_through_tcache_route_jobs() {
    let fixture_name = "tokyo_3_places";
    assert!(PLACE_FIXTURE_NAMES.contains(&fixture_name));
    let fixture = load_place_fixture(fixture_name).expect("fixture setup must be valid");
    let optimize_request =
        build_optimize_request_from_fixture(fixture_name, "real-place-fixture-test")
            .expect("fixture must build a valid troute request");
    let problem =
        OptimizationProblem::try_from(optimize_request).expect("fixture must be a valid problem");
    assert_eq!(problem.locations().len(), fixture.locations.len());

    let mut base_url: Url = env::var("TCACHE_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:3200".to_owned())
        .parse()
        .expect("TCACHE_BASE_URL must be a valid URL");
    if !base_url.path().ends_with('/') {
        base_url.set_path(&format!("{}/", base_url.path()));
    }
    let pair_provider = TcacheTravelTimeProvider::new(TcacheRoutingConfig {
        base_url,
        poll_interval: Duration::from_millis(250),
        timeout: Duration::from_secs(120),
    })
    .expect("tcache provider must initialize");
    let matrix_provider = PairwiseMatrixRoutingProvider::new(pair_provider);
    let context = RoutingContext {
        departure_time: Some(Utc::now() + ChronoDuration::minutes(5)),
        travel_mode: TravelMode::Walking,
        timezone: Some(fixture.timezone.clone()),
        options: [
            ("languageCode".to_owned(), "ja".to_owned()),
            ("regionCode".to_owned(), fixture.region.clone()),
        ]
        .into_iter()
        .collect(),
    };

    let matrix = matrix_provider
        .travel_time_matrix(problem.locations(), &context)
        .unwrap_or_else(|error| {
            let places = fixture
                .locations
                .iter()
                .map(|location| {
                    format!("{} ({}, {})", location.name, location.id, location.place_id)
                })
                .collect::<Vec<_>>()
                .join(", ");
            panic!("fixture={fixture_name}; places=[{places}]; error={error}")
        });

    assert_eq!(matrix.size(), fixture.locations.len());
    for row in 0..matrix.size() {
        for column in 0..matrix.size() {
            let minutes = matrix
                .travel_minutes(row, column)
                .expect("matrix coordinates must be valid");
            if row == column {
                assert_eq!(minutes, 0);
            } else {
                assert!(minutes > 0, "pair {row} -> {column} must have travel time");
            }
        }
    }
}
