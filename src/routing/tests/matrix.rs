#[test]
fn concurrency_one_matches_sequential_directed_pair_order() {
    let provider = PairwiseMatrixRoutingProvider::with_concurrency(
        DirectedPairs::default(),
        NonZeroUsize::new(1).unwrap(),
    );
    let matrix = provider
        .travel_time_matrix(&locations(), &RoutingContext::default())
        .unwrap();
    assert_eq!(matrix.travel_minutes(0, 2), Some(2));
    assert_eq!(matrix.travel_minutes(2, 0), Some(20));
    assert_eq!(matrix.travel_minutes(1, 1), Some(0));
    assert_eq!(
        *provider.inner().calls.lock().unwrap(),
        [
            ("0".to_owned(), "1".to_owned()),
            ("0".to_owned(), "2".to_owned()),
            ("1".to_owned(), "0".to_owned()),
            ("1".to_owned(), "2".to_owned()),
            ("2".to_owned(), "0".to_owned()),
            ("2".to_owned(), "1".to_owned()),
        ]
    );
}

struct ConcurrentPairs {
    active: AtomicUsize,
    max_active: AtomicUsize,
    wave: Barrier,
    completions: Mutex<Vec<(String, String)>>,
}

impl ConcurrentPairs {
    fn new(concurrency: usize) -> Self {
        Self {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            wave: Barrier::new(concurrency),
            completions: Mutex::new(Vec::new()),
        }
    }
}

impl TravelTimeProvider for ConcurrentPairs {
    fn travel_time(
        &self,
        from: &Location,
        to: &Location,
        _context: &RoutingContext,
    ) -> Result<TravelTime, RoutingError> {
        let active = self.active.fetch_add(1, Ordering::AcqRel) + 1;
        self.max_active.fetch_max(active, Ordering::AcqRel);
        self.wave.wait();
        if from.id() == "0" && to.id() == "1" {
            thread::sleep(Duration::from_millis(25));
        }
        self.completions
            .lock()
            .unwrap()
            .push((from.id().to_owned(), to.id().to_owned()));
        self.active.fetch_sub(1, Ordering::AcqRel);
        Ok(TravelTime {
            minutes: pair_minutes(from, to),
        })
    }
}

#[test]
fn bounded_concurrency_preserves_matrix_order_after_out_of_order_completion() {
    let provider = PairwiseMatrixRoutingProvider::with_concurrency(
        ConcurrentPairs::new(3),
        NonZeroUsize::new(3).unwrap(),
    );

    let matrix = provider
        .travel_time_matrix(&locations(), &RoutingContext::default())
        .unwrap();

    assert_eq!(provider.inner().max_active.load(Ordering::Acquire), 3);
    assert_ne!(
        provider.inner().completions.lock().unwrap()[0],
        ("0".to_owned(), "1".to_owned())
    );
    assert_eq!(matrix.travel_minutes(0, 1), Some(1));
    assert_eq!(matrix.travel_minutes(0, 2), Some(2));
    assert_eq!(matrix.travel_minutes(1, 0), Some(10));
    assert_eq!(matrix.travel_minutes(1, 1), Some(0));
    assert_eq!(matrix.travel_minutes(2, 0), Some(20));
    assert_eq!(matrix.travel_minutes(2, 1), Some(21));
}

#[derive(Default)]
struct RecordingRoutingProgress {
    updates: Mutex<Vec<(usize, usize)>>,
}

impl RoutingProgressObserver for RecordingRoutingProgress {
    fn pairs_completed(&self, completed: usize, total: usize) {
        self.updates.lock().unwrap().push((completed, total));
    }
}

#[test]
fn pairwise_matrix_reports_real_monotonic_pair_completions() {
    let provider = PairwiseMatrixRoutingProvider::with_concurrency(
        DirectedPairs::default(),
        NonZeroUsize::new(3).unwrap(),
    );
    let progress = RecordingRoutingProgress::default();

    provider
        .travel_time_matrix_with_progress(&locations(), &RoutingContext::default(), &progress)
        .unwrap();

    assert_eq!(
        *progress.updates.lock().unwrap(),
        [(1, 6), (2, 6), (3, 6), (4, 6), (5, 6), (6, 6)]
    );
}

struct OneUnreachablePair;

impl TravelTimeProvider for OneUnreachablePair {
    fn travel_time(
        &self,
        from: &Location,
        to: &Location,
        _context: &RoutingContext,
    ) -> Result<TravelTime, RoutingError> {
        if from.id() == "2" && to.id() == "0" {
            return Err(RoutingError::Unreachable {
                from: from.id().to_owned(),
                to: to.id().to_owned(),
                reason: "test graph has no reverse arc".to_owned(),
            });
        }
        Ok(TravelTime {
            minutes: pair_minutes(from, to),
        })
    }
}

#[test]
fn provider_unreachable_pair_becomes_a_sparse_arc_not_a_matrix_failure() {
    let provider = PairwiseMatrixRoutingProvider::new(OneUnreachablePair);
    let matrix = provider
        .travel_time_matrix(&locations(), &RoutingContext::default())
        .unwrap();

    assert_eq!(matrix.travel_minutes(0, 1), Some(1));
    assert_eq!(matrix.travel_minutes(2, 0), None);
}
