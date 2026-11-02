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

use super::{
    NoopRoutingProgressObserver, RoutingContext, RoutingError, RoutingProgressObserver,
    RoutingProvider, TravelTimeProvider,
};

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

impl<P: TravelTimeProvider + Sync> RoutingProvider for PairwiseMatrixRoutingProvider<P> {
    fn travel_time_matrix(
        &self,
        locations: &[Location],
        context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let deadline = Instant::now()
            .checked_add(self.total_timeout)
            .ok_or_else(|| {
                RoutingError::Provider(
                    "matrix total timeout exceeds the supported range".to_owned(),
                )
            })?;
        self.build_matrix_until(
            locations,
            context,
            deadline,
            &NoopRoutingProgressObserver,
            &CancellationToken::new(),
        )
    }

    fn travel_time_matrix_with_progress(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        observer: &dyn RoutingProgressObserver,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        let deadline = Instant::now()
            .checked_add(self.total_timeout)
            .ok_or_else(|| {
                RoutingError::Provider(
                    "matrix total timeout exceeds the supported range".to_owned(),
                )
            })?;
        self.build_matrix_until(
            locations,
            context,
            deadline,
            observer,
            &CancellationToken::new(),
        )
    }

    fn travel_time_matrix_until(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        caller_deadline: Instant,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        self.build_matrix_until(
            locations,
            context,
            caller_deadline,
            &NoopRoutingProgressObserver,
            &CancellationToken::new(),
        )
    }

    fn travel_time_matrix_with_control(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        observer: &dyn RoutingProgressObserver,
        cancellation: &CancellationToken,
        caller_deadline: Option<Instant>,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        if cancellation.is_cancelled() {
            return Err(RoutingError::Cancelled);
        }
        let deadline = match caller_deadline {
            Some(deadline) => deadline,
            None => Instant::now()
                .checked_add(self.total_timeout)
                .ok_or_else(|| {
                    RoutingError::Provider(
                        "matrix total timeout exceeds the supported range".to_owned(),
                    )
                })?,
        };
        self.build_matrix_until(locations, context, deadline, observer, cancellation)
    }
}

struct PairFailure {
    from: String,
    to: String,
    cause: RoutingError,
}

/// Owns the mutable state shared by pair workers and the deadline watchdog.
/// Result slots remain indexed by the deterministic directed-pair ordering.
struct MatrixBuildState {
    next_pair: AtomicUsize,
    completed_pairs: AtomicUsize,
    progress_lock: Mutex<()>,
    matrix_timed_out: AtomicBool,
    caller_cancelled: AtomicBool,
    cancellation: CancellationToken,
    results: Mutex<Vec<Option<Option<u32>>>>,
    first_error: Mutex<Option<PairFailure>>,
}

impl MatrixBuildState {
    fn new(pair_count: usize) -> Self {
        Self {
            next_pair: AtomicUsize::new(0),
            completed_pairs: AtomicUsize::new(0),
            progress_lock: Mutex::new(()),
            matrix_timed_out: AtomicBool::new(false),
            caller_cancelled: AtomicBool::new(false),
            cancellation: CancellationToken::new(),
            results: Mutex::new(vec![None; pair_count]),
            first_error: Mutex::new(None),
        }
    }

    fn next_pair(&self) -> usize {
        self.next_pair.fetch_add(1, Ordering::Relaxed)
    }

    fn complete(
        &self,
        pair_index: usize,
        minutes: Option<u32>,
        observer: &dyn RoutingProgressObserver,
        total_pairs: usize,
    ) {
        self.results.lock().expect("pair result lock poisoned")[pair_index] = Some(minutes);
        let _progress_guard = self
            .progress_lock
            .lock()
            .expect("routing progress lock poisoned");
        let completed = self.completed_pairs.fetch_add(1, Ordering::AcqRel) + 1;
        observer.pairs_completed(completed, total_pairs);
    }

    fn fail(&self, failure: PairFailure, timed_out: bool) {
        if timed_out {
            self.matrix_timed_out.store(true, Ordering::Release);
        }
        let mut first_error = self.first_error.lock().expect("pair error lock poisoned");
        if first_error.is_none() {
            *first_error = Some(failure);
            self.cancellation.cancel();
        }
    }
}

impl<P: TravelTimeProvider + Sync> PairwiseMatrixRoutingProvider<P> {
    fn build_matrix_until(
        &self,
        locations: &[Location],
        context: &RoutingContext,
        caller_deadline: Instant,
        observer: &dyn RoutingProgressObserver,
        caller_cancellation: &CancellationToken,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        if caller_cancellation.is_cancelled() {
            return Err(RoutingError::Cancelled);
        }
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
        let own_deadline = started.checked_add(self.total_timeout).ok_or_else(|| {
            RoutingError::Provider("matrix total timeout exceeds the supported range".to_owned())
        })?;
        let deadline = caller_deadline.min(own_deadline);
        // Outer `None` means unfinished; `Some(None)` is a successfully
        // classified unreachable arc.
        let state = MatrixBuildState::new(pairs.len());
        let worker_count = self.max_concurrency.get().min(pairs.len());
        let (finished_tx, finished_rx) = mpsc::sync_channel(1);

        thread::scope(|scope| {
            let watchdog_cancellation = &state.cancellation;
            let watchdog_timed_out = &state.matrix_timed_out;
            let watchdog_caller_cancelled = &state.caller_cancelled;
            let watchdog = scope.spawn(move || loop {
                if caller_cancellation.is_cancelled() {
                    watchdog_caller_cancelled.store(true, Ordering::Release);
                    watchdog_cancellation.cancel();
                    break;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    watchdog_timed_out.store(true, Ordering::Release);
                    watchdog_cancellation.cancel();
                    break;
                }
                match finished_rx.recv_timeout(remaining.min(Duration::from_millis(10))) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            });
            let mut workers = Vec::with_capacity(worker_count);
            for _ in 0..worker_count {
                workers.push(scope.spawn(|| loop {
                    if state.cancellation.is_cancelled() {
                        break;
                    }
                    let pair_index = state.next_pair();
                    let Some(&(from_index, to_index)) = pairs.get(pair_index) else {
                        break;
                    };
                    if state.cancellation.is_cancelled() || Instant::now() >= deadline {
                        if Instant::now() >= deadline {
                            state.fail(
                                PairFailure {
                                    from: locations[from_index].id().to_owned(),
                                    to: locations[to_index].id().to_owned(),
                                    cause: RoutingError::Provider(
                                        "matrix deadline reached before pair query started"
                                            .to_owned(),
                                    ),
                                },
                                true,
                            );
                        }
                        break;
                    }

                    match self.provider.travel_time_with_cancellation_until(
                        &locations[from_index],
                        &locations[to_index],
                        context,
                        &state.cancellation,
                        deadline,
                    ) {
                        Ok(travel_time) => {
                            if Instant::now() >= deadline {
                                state.fail(
                                    PairFailure {
                                        from: locations[from_index].id().to_owned(),
                                        to: locations[to_index].id().to_owned(),
                                        cause: RoutingError::Provider(
                                            "pair completed after matrix deadline".to_owned(),
                                        ),
                                    },
                                    true,
                                );
                                break;
                            }
                            state.complete(
                                pair_index,
                                Some(travel_time.minutes),
                                observer,
                                pairs.len(),
                            );
                        }
                        Err(RoutingError::Unreachable { .. }) => {
                            state.complete(pair_index, None, observer, pairs.len());
                        }
                        Err(error) => {
                            state.fail(
                                PairFailure {
                                    from: locations[from_index].id().to_owned(),
                                    to: locations[to_index].id().to_owned(),
                                    cause: error,
                                },
                                Instant::now() >= deadline,
                            );
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

        if state.caller_cancelled.load(Ordering::Acquire) || caller_cancellation.is_cancelled() {
            return Err(RoutingError::Cancelled);
        }

        let mut failure = state
            .first_error
            .into_inner()
            .expect("pair error lock poisoned");
        if failure.is_none()
            && state.matrix_timed_out.load(Ordering::Acquire)
            && state.completed_pairs.load(Ordering::Acquire) < pairs.len()
        {
            let missing_pair_index = state
                .results
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
                matrix_timeout: state.matrix_timed_out.load(Ordering::Acquire),
                elapsed_ms,
                completed_pairs: state.completed_pairs.load(Ordering::Acquire),
                total_pairs: pairs.len(),
                failed_from: failure.from,
                failed_to: failure.to,
                source: Box::new(failure.cause),
            });
        }

        let mut rows = vec![vec![None; locations.len()]; locations.len()];
        for (index, row) in rows.iter_mut().enumerate() {
            row[index] = Some(0);
        }
        for ((from_index, to_index), minutes) in pairs.into_iter().zip(
            state
                .results
                .into_inner()
                .expect("pair result lock poisoned"),
        ) {
            rows[from_index][to_index] =
                minutes.expect("every pair must complete when no provider error occurred");
        }
        TravelTimeMatrix::new_sparse(rows).map_err(RoutingError::InvalidMatrix)
    }
}
