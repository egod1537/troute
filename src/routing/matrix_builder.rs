use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{cancellation::CancellationToken, domain::Location, matrix::TravelTimeMatrix};

use super::{RoutingContext, RoutingError, RoutingProvider, TravelTimeProvider};

pub const DEFAULT_PAIR_CONCURRENCY: usize = 4;
pub const DEFAULT_MATRIX_TOTAL_TIMEOUT_MS: u64 = 120_000;

/// Adapts a pair-query provider into the matrix boundary used by solvers.
#[derive(Debug, Clone)]
pub struct PairwiseMatrixRoutingProvider<P> {
    provider: P,
    max_concurrency: NonZeroUsize,
    total_timeout: Duration,
}

impl<P> PairwiseMatrixRoutingProvider<P> {
    pub fn new(provider: P) -> Self {
        Self::with_concurrency(
            provider,
            NonZeroUsize::new(DEFAULT_PAIR_CONCURRENCY)
                .expect("default pair concurrency must be positive"),
        )
    }

    pub fn with_concurrency(provider: P, max_concurrency: NonZeroUsize) -> Self {
        Self::with_limits(
            provider,
            max_concurrency,
            Duration::from_millis(DEFAULT_MATRIX_TOTAL_TIMEOUT_MS),
        )
    }

    pub fn with_limits(
        provider: P,
        max_concurrency: NonZeroUsize,
        total_timeout: Duration,
    ) -> Self {
        assert!(!total_timeout.is_zero(), "matrix timeout must be positive");
        Self {
            provider,
            max_concurrency,
            total_timeout,
        }
    }

    pub fn inner(&self) -> &P {
        &self.provider
    }

    pub fn max_concurrency(&self) -> usize {
        self.max_concurrency.get()
    }

    pub fn total_timeout(&self) -> Duration {
        self.total_timeout
    }
}

struct PairFailure {
    from: String,
    to: String,
    cause: RoutingError,
}

impl<P: TravelTimeProvider + Sync> RoutingProvider for PairwiseMatrixRoutingProvider<P> {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let pairs = (0..locations.len())
            .flat_map(|from_index| {
                (0..locations.len())
                    .filter(move |&to_index| from_index != to_index)
                    .map(move |to_index| (from_index, to_index))
            })
            .collect::<Vec<_>>();
        if pairs.is_empty() {
            return TravelTimeMatrix::new(vec![vec![0; locations.len()]; locations.len()])
                .map_err(RoutingError::InvalidMatrix);
        }

        let started = Instant::now();
        let deadline = started.checked_add(self.total_timeout).ok_or_else(|| {
            RoutingError::Provider("matrix total timeout exceeds the supported range".to_owned())
        })?;
        let next_pair = AtomicUsize::new(0);
        let completed_pairs = AtomicUsize::new(0);
        let matrix_timed_out = AtomicBool::new(false);
        let cancellation = CancellationToken::new();
        let results = Mutex::new(vec![None; pairs.len()]);
        let first_error = Mutex::new(None);
        let worker_count = self.max_concurrency.get().min(pairs.len());
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);

        thread::scope(|scope| {
            let watchdog_cancellation = &cancellation;
            let watchdog_timed_out = &matrix_timed_out;
            let watchdog = scope.spawn(move || {
                if finished_rx
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .is_err()
                {
                    watchdog_timed_out.store(true, Ordering::Release);
                    watchdog_cancellation.cancel();
                }
            });
            let mut workers = Vec::with_capacity(worker_count);
            for _ in 0..worker_count {
                workers.push(scope.spawn(|| loop {
                    if cancellation.is_cancelled() {
                        break;
                    }
                    let pair_index = next_pair.fetch_add(1, Ordering::Relaxed);
                    let Some(&(from_index, to_index)) = pairs.get(pair_index) else {
                        break;
                    };
                    if cancellation.is_cancelled() || Instant::now() >= deadline {
                        if Instant::now() >= deadline {
                            matrix_timed_out.store(true, Ordering::Release);
                            let mut first_error =
                                first_error.lock().expect("pair error lock poisoned");
                            if first_error.is_none() {
                                *first_error = Some(PairFailure {
                                    from: locations[from_index].id().to_owned(),
                                    to: locations[to_index].id().to_owned(),
                                    cause: RoutingError::Provider(
                                        "matrix deadline reached before pair query started"
                                            .to_owned(),
                                    ),
                                });
                            }
                            cancellation.cancel();
                        }
                        break;
                    }

                    match self.provider.travel_time_with_cancellation_until(
                        &locations[from_index],
                        &locations[to_index],
                        context,
                        &cancellation,
                        deadline,
                    ) {
                        Ok(travel_time) => {
                            if Instant::now() >= deadline {
                                matrix_timed_out.store(true, Ordering::Release);
                                let mut first_error =
                                    first_error.lock().expect("pair error lock poisoned");
                                if first_error.is_none() {
                                    *first_error = Some(PairFailure {
                                        from: locations[from_index].id().to_owned(),
                                        to: locations[to_index].id().to_owned(),
                                        cause: RoutingError::Provider(
                                            "pair completed after matrix deadline".to_owned(),
                                        ),
                                    });
                                }
                                cancellation.cancel();
                                break;
                            }
                            results.lock().expect("pair result lock poisoned")[pair_index] =
                                Some(travel_time.minutes);
                            completed_pairs.fetch_add(1, Ordering::AcqRel);
                        }
                        Err(error) => {
                            if Instant::now() >= deadline {
                                matrix_timed_out.store(true, Ordering::Release);
                            }
                            let mut first_error =
                                first_error.lock().expect("pair error lock poisoned");
                            if first_error.is_none() {
                                *first_error = Some(PairFailure {
                                    from: locations[from_index].id().to_owned(),
                                    to: locations[to_index].id().to_owned(),
                                    cause: error,
                                });
                                cancellation.cancel();
                            }
                            break;
                        }
                    }
                }));
            }
            for worker in workers {
                worker.join().expect("pair worker panicked");
            }
            let _ = finished_tx.send(());
            watchdog.join().expect("matrix timeout watchdog panicked");
        });

        let mut failure = first_error.into_inner().expect("pair error lock poisoned");
        if failure.is_none()
            && matrix_timed_out.load(Ordering::Acquire)
            && completed_pairs.load(Ordering::Acquire) < pairs.len()
        {
            let missing_pair_index = results
                .lock()
                .expect("pair result lock poisoned")
                .iter()
                .position(Option::is_none)
                .expect("an incomplete matrix must have a missing pair");
            let (from_index, to_index) = pairs[missing_pair_index];
            failure = Some(PairFailure {
                from: locations[from_index].id().to_owned(),
                to: locations[to_index].id().to_owned(),
                cause: RoutingError::Provider("matrix deadline reached".to_owned()),
            });
        }
        if let Some(failure) = failure {
            let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            return Err(RoutingError::MatrixBuild {
                matrix_timeout: matrix_timed_out.load(Ordering::Acquire),
                elapsed_ms,
                completed_pairs: completed_pairs.load(Ordering::Acquire),
                total_pairs: pairs.len(),
                failed_from: failure.from,
                failed_to: failure.to,
                source: Box::new(failure.cause),
            });
        }

        let mut rows = vec![vec![0; locations.len()]; locations.len()];
        for ((from_index, to_index), minutes) in pairs
            .into_iter()
            .zip(results.into_inner().expect("pair result lock poisoned"))
        {
            rows[from_index][to_index] =
                minutes.expect("every pair must complete when no provider error occurred");
        }
        TravelTimeMatrix::new(rows).map_err(RoutingError::InvalidMatrix)
    }
}
