use super::*;
use axum::http::HeaderValue;

fn entry(recorder: &JobObservationRecorder, pair_id: &str) -> JobTimelineEntry {
    recorder.entry(
        pair_id,
        ObservationDirection::Request,
        ObservationPeer::Testbed,
        ObservationPeer::Troute,
    )
}

#[test]
fn wire_values_are_stable() {
    assert_eq!(
        serde_json::to_value(ObservationDirection::Response).unwrap(),
        "RESPONSE"
    );
    assert_eq!(
        serde_json::to_value(ObservationPeer::Troute).unwrap(),
        "troute"
    );
}

#[test]
fn entry_and_job_bounds_evict_oldest_values() {
    let store = Arc::new(InMemoryJobTimelineStore::new(2, 2));
    let recorder = JobObservationRecorder::new(store.clone());

    for pair_id in ["one", "two", "three"] {
        store.append("job-a", entry(&recorder, pair_id));
    }
    assert_eq!(
        store
            .list("job-a")
            .iter()
            .map(|entry| entry.pair_id.as_str())
            .collect::<Vec<_>>(),
        vec!["two", "three"]
    );

    store.append("job-b", entry(&recorder, "four"));
    store.append("job-c", entry(&recorder, "five"));
    assert!(store.list("job-a").is_empty());
    assert_eq!(store.list("job-b").len(), 1);
    assert_eq!(store.list("job-c").len(), 1);
}

#[test]
fn clear_removes_the_job_and_its_eviction_slot() {
    let store = InMemoryJobTimelineStore::new(1, 2);
    let recorder = JobObservationRecorder::in_memory();
    store.append("job-a", entry(&recorder, "one"));
    store.clear("job-a");
    store.append("job-b", entry(&recorder, "two"));

    assert!(store.list("job-a").is_empty());
    assert_eq!(store.list("job-b").len(), 1);
}

#[test]
fn header_allowlist_excludes_credentials_and_cookies() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    headers.insert(header::ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(
        header::AUTHORIZATION,
        HeaderValue::from_static("Bearer secret"),
    );
    headers.insert(header::COOKIE, HeaderValue::from_static("session=secret"));
    headers.insert(
        header::SET_COOKIE,
        HeaderValue::from_static("session=secret"),
    );
    headers.insert("x-api-key", HeaderValue::from_static("secret"));

    let allowed = allowlisted_headers(&headers).unwrap();
    assert_eq!(allowed.len(), 2);
    assert_eq!(allowed["content-type"], "application/json");
    assert_eq!(allowed["accept"], "application/json");
}

#[test]
fn equal_timestamps_retain_insertion_order_in_store() {
    let store = InMemoryJobTimelineStore::default();
    let recorder = JobObservationRecorder::in_memory();
    let mut first = entry(&recorder, "first");
    let mut second = entry(&recorder, "second");
    first.timestamp_ms = 42;
    second.timestamp_ms = 42;
    store.append("job", first);
    store.append("job", second);

    assert_eq!(
        store
            .list("job")
            .iter()
            .map(|entry| entry.pair_id.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}
