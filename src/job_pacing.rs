use std::time::{Duration, Instant};

use crate::{api::DebugOptions, cancellation::CancellationToken, events::ProgressStage};

/// Schedules diagnostic progress visibility without changing optimization data.
#[derive(Debug, Clone)]
pub(crate) struct DebugJobPacer {
    started_at: Instant,
    minimum_duration_ms: Option<u64>,
}

impl DebugJobPacer {
    pub(crate) fn new(debug: Option<&DebugOptions>) -> Self {
        Self {
            started_at: Instant::now(),
            minimum_duration_ms: debug
                .and_then(|options| options.min_job_duration_ms)
                .filter(|duration| *duration > 0),
        }
    }

    pub(crate) fn minimum_duration_ms(&self) -> Option<u64> {
        self.minimum_duration_ms
    }

    pub(crate) async fn wait_for_stage(
        &self,
        stage: ProgressStage,
        cancellation: &CancellationToken,
    ) -> bool {
        let percentage = match stage {
            ProgressStage::Accepted => 0,
            ProgressStage::BuildingMatrix => 20,
            ProgressStage::Solving => 50,
            ProgressStage::Scheduling => 80,
        };
        self.wait_until_percentage(percentage, cancellation).await
    }

    pub(crate) async fn wait_until_minimum(&self, cancellation: &CancellationToken) -> bool {
        self.wait_until_percentage(100, cancellation).await
    }

    async fn wait_until_percentage(
        &self,
        percentage: u64,
        cancellation: &CancellationToken,
    ) -> bool {
        let Some(minimum_duration_ms) = self.minimum_duration_ms else {
            return !cancellation.is_cancelled();
        };
        let target_ms = minimum_duration_ms.saturating_mul(percentage) / 100;
        let deadline = self.started_at + Duration::from_millis(target_ms);
        let now = Instant::now();
        if now >= deadline {
            return !cancellation.is_cancelled();
        }

        tokio::select! {
            biased;
            _ = cancellation.cancelled() => false,
            _ = tokio::time::sleep(deadline.saturating_duration_since(now)) => {
                !cancellation.is_cancelled()
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
}
