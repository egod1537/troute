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
        let percentage = u64::from(stage.range().start);
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
mod tests;
