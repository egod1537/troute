struct FailingPairs {
    started: Barrier,
    calls: Mutex<Vec<(String, String)>>,
    in_flight_cancelled: AtomicBool,
}

impl FailingPairs {
    fn new() -> Self {
        Self {
            started: Barrier::new(2),
            calls: Mutex::new(Vec::new()),
            in_flight_cancelled: AtomicBool::new(false),
        }
    }
}

impl TravelTimeProvider for FailingPairs {
    fn travel_time(
        &self,
        _from: &Location,
        _to: &Location,
        _context: &RoutingContext,
    ) -> Result<TravelTime, RoutingError> {
        unreachable!("pairwise routing uses the cancellation-aware method")
    }

    fn travel_time_with_cancellation(
        &self,
        from: &Location,
        to: &Location,
        _context: &RoutingContext,
        cancellation: &CancellationToken,
    ) -> Result<TravelTime, RoutingError> {
        self.calls
            .lock()
            .unwrap()
            .push((from.id().to_owned(), to.id().to_owned()));
        self.started.wait();
        if to.id() == "1" {
            return Err(RoutingError::Provider("test failure".to_owned()));
        }
        while !cancellation.is_cancelled() {
            thread::yield_now();
        }
        self.in_flight_cancelled.store(true, Ordering::Release);
        Err(RoutingError::Provider("cancelled".to_owned()))
    }
}

#[test]
fn pair_failure_has_context_and_cancels_in_flight_and_unscheduled_work() {
    let provider = PairwiseMatrixRoutingProvider::with_concurrency(
        FailingPairs::new(),
        NonZeroUsize::new(2).unwrap(),
    );

    let error = provider
        .travel_time_matrix(&locations(), &RoutingContext::default())
        .unwrap_err();

    assert!(error.to_string().contains("matrix_timeout=false"));
    assert!(error.to_string().contains("completed_pairs=0/6"));
    assert!(error.to_string().contains("failed_pair=0 -> 1"));
    assert!(provider.inner().in_flight_cancelled.load(Ordering::Acquire));
    assert_eq!(provider.inner().calls.lock().unwrap().len(), 2);
}

struct DeadlinePairs {
    started: AtomicUsize,
    cancelled: AtomicUsize,
}

#[test]
fn caller_cancellation_stops_in_flight_pairs_and_starts_no_more_work() {
    let provider = PairwiseMatrixRoutingProvider::with_limits(
        DeadlinePairs {
            started: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
        },
        NonZeroUsize::new(4).unwrap(),
        Duration::from_secs(5),
    );
    let cancellation = CancellationToken::new();
    let all_locations = locations_n(10);
    let result = thread::scope(|scope| {
        let worker = scope.spawn(|| {
            provider.travel_time_matrix_with_control(
                &all_locations,
                &RoutingContext::default(),
                &NoopRoutingProgressObserver,
                &cancellation,
                None,
            )
        });
        while provider.inner().started.load(Ordering::Acquire) < 2 {
            thread::yield_now();
        }
        cancellation.cancel();
        worker.join().unwrap()
    });

    assert!(matches!(result, Err(RoutingError::Cancelled)));
    let started = provider.inner().started.load(Ordering::Acquire);
    assert!((2..=4).contains(&started));
    assert_eq!(provider.inner().cancelled.load(Ordering::Acquire), started);
    thread::sleep(Duration::from_millis(20));
    assert_eq!(provider.inner().started.load(Ordering::Acquire), started);
}

impl TravelTimeProvider for DeadlinePairs {
    fn travel_time(
        &self,
        _from: &Location,
        _to: &Location,
        _context: &RoutingContext,
    ) -> Result<TravelTime, RoutingError> {
        unreachable!("pairwise routing uses the deadline-aware method")
    }

    fn travel_time_with_cancellation_until(
        &self,
        _from: &Location,
        _to: &Location,
        _context: &RoutingContext,
        cancellation: &CancellationToken,
        _deadline: Instant,
    ) -> Result<TravelTime, RoutingError> {
        self.started.fetch_add(1, Ordering::AcqRel);
        while !cancellation.is_cancelled() {
            thread::yield_now();
        }
        if cancellation.is_cancelled() {
            self.cancelled.fetch_add(1, Ordering::AcqRel);
        }
        Err(RoutingError::Provider("deadline reached".to_owned()))
    }
}

#[test]
fn total_timeout_cancels_running_pairs_and_starts_no_new_pairs() {
    let provider = PairwiseMatrixRoutingProvider::with_limits(
        DeadlinePairs {
            started: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
        },
        NonZeroUsize::new(2).unwrap(),
        Duration::from_millis(20),
    );

    let error = provider
        .travel_time_matrix(&locations(), &RoutingContext::default())
        .unwrap_err();
    let message = error.to_string();

    assert!(message.contains("matrix_timeout=true"), "{message}");
    assert!(message.contains("completed_pairs=0/6"), "{message}");
    assert!(message.contains("failed_pair="), "{message}");
    let started = provider.inner().started.load(Ordering::Acquire);
    assert!((1..=2).contains(&started));
    assert_eq!(provider.inner().cancelled.load(Ordering::Acquire), started);
}

#[test]
fn caller_deadline_shortens_the_pairwise_matrix_timeout() {
    let provider = PairwiseMatrixRoutingProvider::with_limits(
        DeadlinePairs {
            started: AtomicUsize::new(0),
            cancelled: AtomicUsize::new(0),
        },
        NonZeroUsize::new(2).unwrap(),
        Duration::from_secs(5),
    );
    let started = Instant::now();

    let error = provider
        .travel_time_matrix_until(
            &locations(),
            &RoutingContext::default(),
            Instant::now() + Duration::from_millis(20),
        )
        .unwrap_err();

    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(error.to_string().contains("matrix_timeout=true"));
}

#[test]
fn static_provider_validates_location_count() {
    let provider = StaticMatrixRoutingProvider::new(
        TravelTimeMatrix::new(vec![vec![0, 1], vec![2, 0]]).unwrap(),
    );
    assert!(matches!(
        provider.travel_time_matrix(&locations(), &RoutingContext::default()),
        Err(RoutingError::LocationCountMismatch { .. })
    ));
}
