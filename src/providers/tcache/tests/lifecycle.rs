#[test]
fn creates_polls_and_decodes_a_pair_route() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = MockServer::start(vec![
        spec(202, r#"{"jobId":"route-1","status":"queued"}"#),
        spec(200, r#"{"status":"queued"}"#),
        spec(200, r#"{"status":"running"}"#),
        spec(200, r#"{"status":"completed"}"#),
        spec(
            200,
            r#"{"jobId":"route-1","status":"completed","result":{"provider":"google","routes":[{"durationSeconds":61.1}]}}"#,
        ),
    ]);
    let departure_time = "2026-09-23T09:00:00+09:00".parse().unwrap();
    let context = RoutingContext {
        departure_time: Some(departure_time),
        travel_mode: crate::routing::TravelMode::Walking,
        options: [
            ("languageCode".to_owned(), "ko".to_owned()),
            ("regionCode".to_owned(), "KR".to_owned()),
            ("routeProvider".to_owned(), "kakao-maps".to_owned()),
            (
                "unsupported".to_owned(),
                "secret-internal-option".to_owned(),
            ),
        ]
        .into_iter()
        .collect(),
        ..RoutingContext::default()
    };
    let travel = travel_time(&provider(&server, Duration::from_secs(1)), &context).unwrap();
    assert_eq!(travel.minutes, 2);
    let requests = server.requests.lock().unwrap();
    assert!(requests[0].starts_with("POST /api/route/jobs "));
    assert!(requests[0].contains(r#""locations":[{"placeId":"place-A"},{"placeId":"place-B"}]"#));
    assert!(requests[0].contains(r#""mode":"WALKING""#));
    assert!(requests[0].contains(r#""departureTime":"2026-09-23T00:00:00Z""#));
    assert!(requests[0].contains(r#""languageCode":"ko""#));
    assert!(requests[0].contains(r#""regionCode":"KR""#));
    assert!(requests[0].contains(r#""provider":"kakao-maps""#));
    assert!(!requests[0].contains("unsupported"));
    assert!(!requests[0].contains("secret-internal-option"));
    assert!(requests[4].starts_with("GET /api/route/jobs/route-1/result "));
}

#[test]
fn forwards_every_supported_travel_mode_to_route_jobs() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (travel_mode, provider_value) in [
        (crate::routing::TravelMode::Transit, "TRANSIT"),
        (crate::routing::TravelMode::Driving, "DRIVING"),
        (crate::routing::TravelMode::Walking, "WALKING"),
        (crate::routing::TravelMode::Bicycling, "BICYCLING"),
    ] {
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"route-mode"}"#),
            spec(200, r#"{"status":"completed"}"#),
            spec(
                200,
                r#"{"jobId":"route-mode","status":"completed","result":{"routes":[{"durationSeconds":60}]}}"#,
            ),
        ]);
        let context = RoutingContext {
            travel_mode,
            ..RoutingContext::default()
        };

        travel_time(&provider(&server, Duration::from_secs(1)), &context).unwrap();

        let requests = server.requests.lock().unwrap();
        assert!(
            requests[0].contains(&format!(r#""mode":"{provider_value}""#)),
            "request did not contain mode {provider_value}: {}",
            requests[0]
        );
    }
}

#[test]
fn decodes_cached_route_results() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = MockServer::start(vec![
        spec(202, r#"{"jobId":"route-cache"}"#),
        spec(200, r#"{"status":"completed","cache":{"hit":true}}"#),
        spec(
            200,
            r#"{"jobId":"route-cache","status":"completed","cache":{"hit":true},"result":{"routes":[{"durationSeconds":120}]}}"#,
        ),
    ]);
    let travel = travel_time(
        &provider(&server, Duration::from_secs(1)),
        &RoutingContext::default(),
    )
    .unwrap();
    assert_eq!(travel.minutes, 2);
}

#[test]
fn maps_failed_cancelled_and_unknown_jobs_to_routing_errors() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (status, code) in [
        ("failed", "PAIR_ROUTE_FAILED"),
        ("cancelled", "JOB_CANCELLED"),
    ] {
        let body =
            format!(r#"{{"status":"{status}","error":{{"code":"{code}","message":"stopped"}}}}"#);
        let server = MockServer::start(vec![spec(202, r#"{"jobId":"route-2"}"#), spec(200, body)]);
        let error = travel_time(
            &provider(&server, Duration::from_secs(1)),
            &RoutingContext::default(),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains(code),
            "expected {code} in {error}"
        );
        assert!(error.to_string().contains("A -> B"));
    }

    let server = MockServer::start(vec![
        spec(202, r#"{"jobId":"route-unknown"}"#),
        spec(200, r#"{"status":"paused"}"#),
    ]);
    let error = travel_time(
        &provider(&server, Duration::from_secs(1)),
        &RoutingContext::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("unknown status paused"));
}
