mod support;

use std::{
    env, thread,
    time::{Duration, Instant},
};

use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use reqwest::{blocking::Client, Url};
use serde_json::{json, Value};
use support::place_fixtures::{
    build_optimize_request_from_fixture, load_place_fixture, PLACE_FIXTURE_NAMES,
};

#[test]
#[ignore = "requires a live tcache server and billable Google Routes access"]
fn tokyo_fixture_resolves_through_tcache_matrix_job() {
    let fixture_name = "tokyo_3_places";
    assert!(PLACE_FIXTURE_NAMES.contains(&fixture_name));
    let fixture = load_place_fixture(fixture_name).expect("fixture setup must be valid");
    let optimize_request =
        build_optimize_request_from_fixture(fixture_name, "real-place-fixture-test")
            .expect("fixture must build a valid troute request");
    assert_eq!(optimize_request.locations.len(), fixture.locations.len());
    let base_url: Url = env::var("TCACHE_BASE_URL")
        .unwrap_or_else(|_| "http://localhost:3200".to_owned())
        .parse()
        .expect("TCACHE_BASE_URL must be a valid URL");
    let endpoint = |path: &str| {
        let mut base = base_url.clone();
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        base.join(path).expect("tcache endpoint must be valid")
    };
    let client = Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("HTTP client must initialize");
    let request = json!({
        "locations": fixture.locations.iter().map(|location| json!({
            "id": location.id,
            "placeId": location.place_id,
        })).collect::<Vec<_>>(),
        "mode": "WALKING",
        "departureTime": (Utc::now() + ChronoDuration::minutes(5))
            .to_rfc3339_opts(SecondsFormat::Secs, true),
        "options": { "languageCode": "ja", "regionCode": fixture.region },
    });

    let created: Value = client
        .post(endpoint("api/route/matrix/jobs"))
        .json(&request)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .expect("stage=create matrix job")
        .json()
        .expect("stage=decode create response");
    let job_id = created["jobId"]
        .as_str()
        .expect("create response must contain jobId");
    let deadline = Instant::now() + Duration::from_secs(120);

    loop {
        assert!(
            Instant::now() < deadline,
            "fixture={fixture_name}; jobId={job_id}; stage=poll; error=timeout"
        );
        let status: Value = client
            .get(endpoint(&format!("api/route/matrix/jobs/{job_id}")))
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .expect("stage=poll matrix job")
            .json()
            .expect("stage=decode matrix status");
        match status["status"].as_str() {
            Some("completed") => break,
            Some("queued" | "running") => thread::sleep(Duration::from_millis(250)),
            terminal => panic_with_fixture(
                fixture_name,
                job_id,
                &fixture,
                &format!("stage=poll; status={terminal:?}; response={status}"),
            ),
        }
    }

    let result: Value = client
        .get(endpoint(&format!("api/route/matrix/jobs/{job_id}/result")))
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .expect("stage=get matrix result")
        .json()
        .expect("stage=decode matrix result");
    let locations = result["locations"]
        .as_array()
        .expect("result locations must be an array");
    let matrix = result["durationSeconds"]
        .as_array()
        .expect("durationSeconds must be an array");

    assert_eq!(locations.len(), fixture.locations.len(), "jobId={job_id}");
    assert_eq!(matrix.len(), fixture.locations.len(), "jobId={job_id}");
    for (index, row) in matrix.iter().enumerate() {
        let row = row.as_array().expect("matrix row must be an array");
        assert_eq!(row.len(), fixture.locations.len(), "jobId={job_id}");
        assert_eq!(row[index].as_u64(), Some(0), "jobId={job_id}");
        assert!(row.iter().all(Value::is_u64), "jobId={job_id}");
    }
}

fn panic_with_fixture(
    fixture_name: &str,
    job_id: &str,
    fixture: &support::place_fixtures::PlaceFixture,
    error: &str,
) -> ! {
    let places = fixture
        .locations
        .iter()
        .map(|location| format!("{} ({}, {})", location.name, location.id, location.place_id))
        .collect::<Vec<_>>()
        .join(", ");
    panic!("fixture={fixture_name}; jobId={job_id}; places=[{places}]; {error}")
}
