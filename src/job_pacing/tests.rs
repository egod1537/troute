use super::*;

#[tokio::test]
async fn disabled_pacer_returns_immediately() {
    let pacer = DebugJobPacer::new(None);
    let started = Instant::now();
    assert!(pacer.wait_until_minimum(&CancellationToken::new()).await);
    assert!(started.elapsed() < Duration::from_millis(20));
}

#[tokio::test]
async fn cancellation_interrupts_a_debug_wait() {
    let pacer = DebugJobPacer::new(Some(&DebugOptions {
        min_job_duration_ms: Some(60_000),
        shuffle_result_route: None,
        shuffle_seed: None,
    }));
    let cancellation = CancellationToken::new();
    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        trigger.cancel();
    });

    let started = Instant::now();
    assert!(!pacer.wait_until_minimum(&cancellation).await);
    assert!(started.elapsed() < Duration::from_secs(1));
}
