use super::*;

use crate::{
    cancellation::CancellationToken,
    domain::{Location, RoutingReference, TimeOfDay, TimeWindow},
    routing::{
        NoopRoutingProgressObserver, RouteProviderName, RoutingContext, RoutingError,
        RoutingProvider, TravelMode,
    },
};

struct UnusedRoutingProvider;

impl RoutingProvider for UnusedRoutingProvider {
    fn travel_time_matrix(
        &self,
        _locations: &[Location],
        _context: &RoutingContext,
    ) -> Result<TravelTimeMatrix, RoutingError> {
        panic!("a supplied matrix must bypass the routing provider")
    }
}

fn locations() -> Vec<Location> {
    let window = TimeWindow::new(
        TimeOfDay::from_minutes(0).unwrap(),
        TimeOfDay::from_minutes(23 * 60).unwrap(),
    )
    .unwrap();
    ["A", "B"]
        .into_iter()
        .map(|id| {
            Location::new(
                id.to_owned(),
                RoutingReference::GooglePlaceId(format!("place-{id}")),
                window,
                0,
            )
        })
        .collect()
}

#[test]
fn sparse_matrix_distinguishes_unreachable_from_zero_minutes() {
    let matrix = TravelTimeMatrix::new_sparse(vec![
        vec![Some(0), Some(0), None],
        vec![None, Some(0), Some(7)],
        vec![Some(3), None, Some(0)],
    ])
    .unwrap();

    assert_eq!(matrix.travel_minutes(0, 1), Some(0));
    assert_eq!(matrix.travel_minutes(0, 2), None);
    assert_eq!(matrix.travel_minutes(99, 0), None);
}

#[test]
fn sparse_matrix_requires_a_reachable_zero_diagonal() {
    assert!(matches!(
        TravelTimeMatrix::new_sparse(vec![vec![None]]),
        Err(MatrixError::InvalidDiagonal {
            index: 0,
            actual: None
        })
    ));
}

#[test]
fn matrix_service_builds_and_validates_supplied_dense_and_sparse_matrices() {
    let service = MatrixService::new(UnusedRoutingProvider);
    let locations = locations();
    let context = RoutingContext::default();
    let cancellation = CancellationToken::new();

    let dense = service
        .build(
            &locations,
            MatrixSource::Dense(vec![vec![0, 7], vec![9, 0]]),
            &context,
            &NoopRoutingProgressObserver,
            &cancellation,
            None,
        )
        .unwrap();
    assert_eq!(dense.travel_minutes(0, 1), Some(7));

    let sparse = service
        .build(
            &locations,
            MatrixSource::Sparse(vec![vec![Some(0), None], vec![Some(9), Some(0)]]),
            &context,
            &NoopRoutingProgressObserver,
            &cancellation,
            None,
        )
        .unwrap();
    assert_eq!(sparse.travel_minutes(0, 1), None);

    assert!(matches!(
        service.build(
            &locations,
            MatrixSource::Dense(vec![vec![0]]),
            &context,
            &NoopRoutingProgressObserver,
            &cancellation,
            None,
        ),
        Err(RoutingError::LocationCountMismatch {
            matrix: 1,
            locations: 2
        })
    ));
}

#[test]
fn matrix_service_builds_routing_context_and_honors_pre_cancelled_requests() {
    let service = MatrixService::new(UnusedRoutingProvider);
    let departure_time = chrono::DateTime::parse_from_rfc3339("2026-11-02T09:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let context = service.routing_context(
        departure_time,
        TravelMode::Driving,
        Some("Asia/Seoul".to_owned()),
        Some("KR"),
        Some(RouteProviderName::Google),
    );

    assert_eq!(context.departure_time, Some(departure_time));
    assert_eq!(context.travel_mode, TravelMode::Driving);
    assert_eq!(context.timezone.as_deref(), Some("Asia/Seoul"));
    assert_eq!(context.options["countryCode"], "KR");
    assert_eq!(context.options["routeProviderOverride"], "google");

    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        service.build(
            &locations(),
            MatrixSource::Provider,
            &context,
            &NoopRoutingProgressObserver,
            &cancellation,
            None,
        ),
        Err(RoutingError::Cancelled)
    ));
}
