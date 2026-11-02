use super::super::*;

pub(in crate::http) async fn solver_events(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    Query(query): Query<SolverTelemetryQuery>,
) -> Result<Response, ApiError> {
    let store = state.job_store.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "The requested job does not exist.",
            &job_id,
        )
    })?;
    if store
        .get_job(&job_id)
        .map_err(ApiError::from_storage)?
        .is_none()
    {
        return Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "The requested job does not exist.",
            &job_id,
        ));
    }
    let event_store = store.clone();
    let event_job_id = job_id;
    let initial_cursor = query.after.unwrap_or(0);
    let page_limit = query.limit.unwrap_or(250).clamp(1, 1_000);
    let stream = async_stream::stream! {
        let mut cursor = initial_cursor;
        loop {
            let events = match event_store.list_solver_events(&event_job_id, cursor, page_limit) {
                Ok(events) => events,
                Err(error) => {
                    logger().job_operation_failed("solver SSE read", &event_job_id, &error);
                    break;
                }
            };
            for event in events {
                cursor = cursor.max(event.event_sequence);
                yield Ok::<Event, Infallible>(
                    Event::default()
                        .event("solver_event")
                        .id(event.event_sequence.to_string())
                        .data(serde_json::to_string(&event).expect("solver event must serialize"))
                );
            }
            let terminal = event_store
                .get_job(&event_job_id)
                .ok()
                .flatten()
                .is_none_or(|job| !matches!(job.state.status, JobStatus::Pending | JobStatus::Running));
            if terminal {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(state.sse_heartbeat_interval)
                .text("heartbeat"),
        )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(response)
}

pub(in crate::http) async fn job_events(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Result<Response, ApiError> {
    let store = state.job_store.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "The requested job does not exist.",
            &job_id,
        )
    })?;

    // Subscribe before reading the source-of-truth snapshot. Retry the read if
    // an event was published concurrently so buffered progress older than the
    // snapshot is not replayed after it.
    let subscription = state
        .runner
        .as_ref()
        .and_then(|runner| runner.subscribe(&job_id));
    let read_job = || {
        store
            .get_job(&job_id)
            .map_err(ApiError::from_storage)?
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "JOB_NOT_FOUND",
                    "The requested job does not exist.",
                    &job_id,
                )
            })
    };
    let (initial, initial_sequence) = if let Some(subscription) = subscription.as_ref() {
        loop {
            let before = subscription.current_sequence();
            let initial = read_job()?;
            let after = subscription.current_sequence();
            if before == after {
                break (initial, after);
            }
        }
    } else {
        let initial = read_job()?;
        let sequence = store
            .current_event_sequence(&job_id)
            .map_err(ApiError::from_storage)?
            // Compatibility for Job records created before sequence metadata.
            .unwrap_or_else(|| u64::try_from(initial.state.updated_at).unwrap_or(0));
        (initial, sequence)
    };
    let initial_terminal = !matches!(
        initial.state.status,
        JobStatus::Pending | JobStatus::Running
    );
    let snapshot = JobEventEnvelope::from_stored_job(initial_sequence, initial);
    let event_store = store.clone();
    let event_job_id = job_id.clone();

    let stream = async_stream::stream! {
        yield Ok::<Event, Infallible>(sse_event("snapshot", &snapshot));
        if initial_terminal {
            return;
        }

        let Some(mut subscription) = subscription else {
            return;
        };
        let mut cursor = initial_sequence;
        'events: loop {
            match subscription.receiver.recv().await {
                Ok(event) => {
                    if event.envelope.sequence <= cursor {
                        continue;
                    }
                    cursor = event.envelope.sequence;
                    let terminal = matches!(
                        event.kind,
                        JobEventKind::Completed | JobEventKind::Failed | JobEventKind::Cancelled
                    );
                    yield Ok(sse_event(event.kind.as_str(), &event.envelope));
                    if terminal {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let (job, recovered_sequence) = loop {
                        let before = subscription.current_sequence();
                        let job = match event_store.get_job(&event_job_id) {
                            Ok(Some(job)) => job,
                            Ok(None) => break 'events,
                            Err(error) => {
                                logger().job_operation_failed(
                                    "SSE lag recovery",
                                    &event_job_id,
                                    &error,
                                );
                                break 'events;
                            }
                        };
                        let after = subscription.current_sequence();
                        if before == after {
                            break (job, after);
                        }
                    };
                    let terminal = !matches!(
                        job.state.status,
                        JobStatus::Pending | JobStatus::Running
                    );
                    cursor = recovered_sequence;
                    let snapshot = JobEventEnvelope::from_stored_job(cursor, job);
                    yield Ok(sse_event("snapshot", &snapshot));
                    if terminal {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    let mut response = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(state.sse_heartbeat_interval)
                .text("heartbeat"),
        )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
        .headers_mut()
        .insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    response.headers_mut().insert(
        header::HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    Ok(response)
}

fn sse_event(event: &'static str, envelope: &JobEventEnvelope) -> Event {
    assert!(
        envelope.is_valid_for_event(event),
        "persisted Job violates the {event} SSE contract"
    );
    Event::default()
        .event(event)
        .id(envelope.sequence.to_string())
        .data(serde_json::to_string(envelope).expect("job event envelope must serialize"))
}
