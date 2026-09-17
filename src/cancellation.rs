use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::watch;

/// Cheap cancellation signal with synchronous checks and async notification.
#[derive(Debug, Clone)]
pub struct CancellationToken {
    inner: Arc<CancellationState>,
}

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    changed: watch::Sender<bool>,
}

impl Default for CancellationToken {
    fn default() -> Self {
        let (changed, _) = watch::channel(false);
        Self {
            inner: Arc::new(CancellationState {
                cancelled: AtomicBool::new(false),
                changed,
            }),
        }
    }
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::AcqRel) {
            self.inner.changed.send_replace(true);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        let mut changed = self.inner.changed.subscribe();
        if *changed.borrow_and_update() {
            return;
        }
        while changed.changed().await.is_ok() {
            if *changed.borrow_and_update() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}
