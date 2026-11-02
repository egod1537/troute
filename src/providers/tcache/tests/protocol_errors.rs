#[test]
fn preserves_tcache_provider_configuration_error_types() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (code, expected_capability_error) in [
        ("PROVIDER_NOT_CONFIGURED", false),
        ("UNSUPPORTED_PROVIDER_CAPABILITY", true),
    ] {
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"route-provider-error"}"#),
            spec(
                200,
                format!(
                    r#"{{"status":"failed","error":{{"code":"{code}","message":"provider setup failed"}}}}"#
                ),
            ),
        ]);
        let context = RoutingContext {
            travel_mode: crate::routing::TravelMode::Transit,
            options: [("routeProvider".to_owned(), "ekispert".to_owned())]
                .into_iter()
                .collect(),
            ..RoutingContext::default()
        };
        let error = travel_time(&provider(&server, Duration::from_secs(1)), &context).unwrap_err();
        if expected_capability_error {
            assert!(matches!(
                error,
                RoutingError::UnsupportedProviderCapability { ref provider, ref mode }
                    if provider == "ekispert" && mode == "TRANSIT"
            ));
        } else {
            assert!(matches!(
                error,
                RoutingError::ProviderNotConfigured { ref provider, ref reason }
                    if provider == "ekispert" && reason == "provider setup failed"
            ));
        }
    }
}

#[test]
fn reports_malformed_create_status_and_result_responses() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for responses in [
        vec![spec(202, "not-json")],
        vec![
            spec(202, r#"{"jobId":"route-status"}"#),
            spec(200, r#"{"stage":"running"}"#),
        ],
        vec![
            spec(202, r#"{"jobId":"route-result"}"#),
            spec(200, r#"{"status":"completed"}"#),
            spec(200, "not-json"),
        ],
    ] {
        let server = MockServer::start(responses);
        let error = travel_time(
            &provider(&server, Duration::from_secs(1)),
            &RoutingContext::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot decode tcache JSON"));
    }
}

#[test]
fn reports_empty_job_id_and_non_success_responses() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let empty = MockServer::start(vec![spec(202, r#"{"jobId":""}"#)]);
    let error = travel_time(
        &provider(&empty, Duration::from_secs(1)),
        &RoutingContext::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("empty jobId"));

    let unavailable = MockServer::start(vec![spec(
        503,
        r#"{"error":{"code":"PROVIDER_ERROR","message":"offline"}}"#,
    )]);
    let error = travel_time(
        &provider(&unavailable, Duration::from_secs(1)),
        &RoutingContext::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("HTTP 503"));
    assert!(error.to_string().contains("PROVIDER_ERROR"));
}

#[test]
fn reports_unreachable_tcache_with_pair_context() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let provider = TcacheTravelTimeProvider::new(TcacheRoutingConfig {
        base_url: Url::parse(&format!("http://{address}/")).unwrap(),
        poll_interval: Duration::from_millis(10),
        timeout: Duration::from_millis(100),
    })
    .unwrap();
    let error = travel_time(&provider, &RoutingContext::default()).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("tcache HTTP request") || message.contains("timed out"),
        "{message}"
    );
    assert!(message.contains("A -> B"));
}

#[test]
fn rejects_missing_and_invalid_route_durations() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for (result, expected) in [
        (
            r#"{"jobId":"route-3","status":"completed","result":{"routes":[]}}"#,
            "missing route durationSeconds",
        ),
        (
            r#"{"jobId":"route-3","status":"completed","result":{"routes":[{"durationSeconds":null}]}}"#,
            "missing route durationSeconds",
        ),
        (
            r#"{"jobId":"route-3","status":"completed","result":{"routes":[{"durationSeconds":-1}]}}"#,
            "invalid route durationSeconds",
        ),
    ] {
        let server = MockServer::start(vec![
            spec(202, r#"{"jobId":"route-3"}"#),
            spec(200, r#"{"status":"completed"}"#),
            spec(200, result),
        ]);
        let error = travel_time(
            &provider(&server, Duration::from_secs(1)),
            &RoutingContext::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains(expected), "{error}");
    }
}
