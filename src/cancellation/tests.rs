use super::*;

#[test]
fn clones_share_cancellation_state() {
    let token = CancellationToken::new();
    let clone = token.clone();
    assert!(!clone.is_cancelled());
    token.cancel();
    assert!(clone.is_cancelled());
}

#[tokio::test]
async fn async_wait_observes_prior_and_future_cancellation() {
    let prior = CancellationToken::new();
    prior.cancel();
    prior.cancelled().await;

    let future = CancellationToken::new();
    let waiter = future.clone();
    let task = tokio::spawn(async move { waiter.cancelled().await });
    tokio::task::yield_now().await;
    future.cancel();
    task.await.unwrap();
}
