#[test]
fn times_out_and_attempts_to_cancel_the_tcache_job() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut responses = vec![spec(202, r#"{"jobId":"route-timeout"}"#)];
    responses.extend((0..30).map(|_| spec(200, r#"{"status":"running"}"#)));
    responses.push(spec(
        200,
        r#"{"jobId":"route-timeout","status":"cancelled"}"#,
    ));
    let server = MockServer::start(responses);
    let error = travel_time(
        &provider(&server, Duration::from_millis(100)),
        &RoutingContext::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("timed out"));
    std::thread::sleep(Duration::from_millis(100));
    assert!(server
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| request.starts_with("POST /api/route/jobs/route-timeout/cancel ")));
}

#[test]
fn cooperative_cancellation_attempts_to_cancel_the_tcache_job() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = MockServer::start(vec![
        spec(202, r#"{"jobId":"route-cancel"}"#),
        spec(200, r#"{"status":"running"}"#),
        spec(200, r#"{"jobId":"route-cancel","status":"cancelled"}"#),
    ]);
    let provider = provider(&server, Duration::from_secs(1));
    let cancellation = CancellationToken::new();
    let [from, to] = locations();

    std::thread::scope(|scope| {
        let request = scope.spawn(|| {
            provider.travel_time_with_cancellation(
                &from,
                &to,
                &RoutingContext::default(),
                &cancellation,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while server.requests.lock().unwrap().len() < 2 {
            assert!(Instant::now() < deadline, "status request did not start");
            std::thread::sleep(Duration::from_millis(1));
        }
        cancellation.cancel();

        let error = request.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    });

    assert!(server
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| request.starts_with("POST /api/route/jobs/route-cancel/cancel ")));
}

#[test]
fn matrix_total_timeout_precedes_pair_timeout_and_cancels_the_job() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let server = MockServer::start(vec![
        spec(202, r#"{"jobId":"route-total-timeout"}"#),
        ResponseSpec {
            status: 200,
            body: r#"{"status":"running"}"#.to_owned(),
            delay: Duration::from_millis(80),
        },
        spec(
            200,
            r#"{"jobId":"route-total-timeout","status":"cancelled"}"#,
        ),
    ]);
    let pair_provider = provider(&server, Duration::from_secs(1));
    let matrix_provider = PairwiseMatrixRoutingProvider::with_limits(
        pair_provider,
        NonZeroUsize::new(1).unwrap(),
        Duration::from_millis(30),
    );

    let error = matrix_provider
        .travel_time_matrix(&locations(), &RoutingContext::default())
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("matrix_timeout=true"), "{message}");
    assert!(message.contains("completed_pairs=0/2"), "{message}");
    assert!(message.contains("failed_pair=A -> B"), "{message}");
    std::thread::sleep(Duration::from_millis(100));
    assert!(server
        .requests
        .lock()
        .unwrap()
        .iter()
        .any(|request| request.starts_with("POST /api/route/jobs/route-total-timeout/cancel ")));
}

#[test]
fn pair_timeout_precedes_matrix_total_timeout() {
    let _guard = TCACHE_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut responses = vec![spec(202, r#"{"jobId":"route-pair-timeout"}"#)];
    responses.extend((0..30).map(|_| spec(200, r#"{"status":"running"}"#)));
    responses.push(spec(
        200,
        r#"{"jobId":"route-pair-timeout","status":"cancelled"}"#,
    ));
    let server = MockServer::start(responses);
    let pair_provider = provider(&server, Duration::from_millis(100));
    let matrix_provider = PairwiseMatrixRoutingProvider::with_limits(
        pair_provider,
        NonZeroUsize::new(1).unwrap(),
        Duration::from_millis(500),
    );

    let error = matrix_provider
        .travel_time_matrix(&locations(), &RoutingContext::default())
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("matrix_timeout=false"), "{message}");
    assert!(message.contains("timed out"), "{message}");
    assert!(message.contains("failed_pair=A -> B"), "{message}");
}
