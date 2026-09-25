mod context;
mod matrix_builder;
mod provider_policy;
mod static_matrix;
mod traits;

pub use context::*;
pub use matrix_builder::*;
pub use provider_policy::*;
pub use static_matrix::*;
pub use traits::*;

#[cfg(test)]
mod tests {
    use std::{
        num::NonZeroUsize,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Barrier, Mutex,
        },
        thread,
        time::{Duration, Instant},
    };

    use super::*;
    use crate::{
        cancellation::CancellationToken,
        domain::{Location, RoutingReference, TimeOfDay, TimeWindow},
        matrix::TravelTimeMatrix,
    };

    fn locations() -> Vec<Location> {
        (0..3)
            .map(|index| {
                Location::new(
                    index.to_string(),
                    RoutingReference::GooglePlaceId(index.to_string()),
                    TimeWindow::new(
                        TimeOfDay::from_minutes(0).unwrap(),
                        TimeOfDay::from_minutes(1439).unwrap(),
                    )
                    .unwrap(),
                    0,
                )
            })
            .collect()
    }

    fn pair_minutes(from: &Location, to: &Location) -> u32 {
        from.id().parse::<u32>().unwrap() * 10 + to.id().parse::<u32>().unwrap()
    }

    #[derive(Default)]
    struct DirectedPairs {
        calls: Mutex<Vec<(String, String)>>,
    }

    impl TravelTimeProvider for DirectedPairs {
        fn travel_time(
            &self,
            from: &Location,
            to: &Location,
            _context: &RoutingContext,
        ) -> Result<TravelTime, RoutingError> {
            self.calls
                .lock()
                .unwrap()
                .push((from.id().to_owned(), to.id().to_owned()));
            Ok(TravelTime {
                minutes: pair_minutes(from, to),
            })
        }
    }

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
}
