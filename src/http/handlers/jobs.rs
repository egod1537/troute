use super::super::*;

pub(in crate::http) async fn submit_job(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<OptimizeRouteRequest>, JsonRejection>,
) -> Result<impl IntoResponse, ApiError> {
    let Json(mut request) = request.map_err(ApiError::from_json_rejection)?;
    state.executor.apply_request_defaults(&mut request);
    OptimizationProblem::try_from(request.clone())
        .map_err(OptimizationServiceError::InvalidRequest)
        .map_err(ApiError::from_service)?;
    enforce_portfolio_experiment(&state, &request)?;
    enforce_routing_pair_limit(&request, state.max_routing_pairs)?;
    let runner = state.runner.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "JOB_STORE_UNAVAILABLE",
            "Asynchronous job submission is unavailable.",
            "the application was built without a job store",
        )
    })?;
    let job_id = request.job_id.clone();
    let started = Instant::now();
    let pair_id = state
        .observation
        .as_ref()
        .map(|observation| observation.next_pair_id("submit"));
    let submitted_state = runner
        .submit(request.clone())
        .map_err(ApiError::from_storage)?;
    if let (Some(observation), Some(pair_id)) = (&state.observation, &pair_id) {
        let mut entry = observation.entry(
            pair_id,
            ObservationDirection::Request,
            ObservationPeer::Testbed,
            ObservationPeer::Troute,
        );
        entry.method = Some("POST".to_owned());
        entry.path = Some("/integration/jobs".to_owned());
        entry.headers = allowlisted_headers(&headers);
        entry.body = serde_json::to_value(&request).ok();
        observation.append(&job_id, entry);
    }

    let response = SubmitJobResponse {
        job_id: submitted_state.job_id,
        status: submitted_state.status,
        created_at: submitted_state.created_at,
    };
    record_submit_response(
        state.observation.as_ref(),
        &job_id,
        pair_id.as_deref(),
        started,
        &response,
    );
    Ok((StatusCode::ACCEPTED, Json(response)))
}

pub(in crate::http) async fn list_jobs(
    State(state): State<AppState>,
    Query(query): Query<JobListQuery>,
) -> Result<Json<JobListResponse>, ApiError> {
    let limit = query.limit.unwrap_or(50).min(500);
    let jobs = match &state.job_store {
        Some(store) => store.list_recent(limit).map_err(ApiError::from_storage)?,
        None => Vec::new(),
    };
    Ok(Json(JobListResponse { jobs }))
}

pub(in crate::http) async fn get_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<StoredJob>, ApiError> {
    let started = Instant::now();
    let job = match &state.job_store {
        Some(store) => store.get_job(&job_id).map_err(ApiError::from_storage)?,
        None => None,
    };
    let job = job.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "The requested job does not exist.",
            job_id.as_str(),
        )
    })?;
    record_job_inspection(state.observation.as_ref(), &job_id, &headers, started, &job);
    Ok(Json(job))
}

pub(in crate::http) async fn solver_telemetry(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    Query(query): Query<SolverTelemetryQuery>,
) -> Result<Json<SolverTelemetryResponse>, ApiError> {
    let store = state.job_store.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "The requested job does not exist.",
            &job_id,
        )
    })?;
    let job = store
        .get_job(&job_id)
        .map_err(ApiError::from_storage)?
        .ok_or_else(|| {
            ApiError::new(
                StatusCode::NOT_FOUND,
                "JOB_NOT_FOUND",
                "The requested job does not exist.",
                &job_id,
            )
        })?;
    let after = query.after.unwrap_or(0);
    let limit = query.limit.unwrap_or(250).clamp(1, 1_000);
    let mut events = store
        .list_solver_events(&job_id, after, limit + 1)
        .map_err(ApiError::from_storage)?;
    let mut summary = job
        .result
        .as_ref()
        .and_then(|response| response.solver_telemetry.clone());
    if events.is_empty() {
        if let Some(stored) = &summary {
            events = stored
                .events
                .iter()
                .filter(|event| event.event_sequence > after)
                .take(limit + 1)
                .cloned()
                .collect();
        }
    }
    let has_more = events.len() > limit;
    events.truncate(limit);
    let next_after = events.last().map_or(after, |event| event.event_sequence);
    if let Some(summary) = &mut summary {
        summary.events.clear();
    }
    Ok(Json(SolverTelemetryResponse {
        schema_version: 1,
        job_id,
        collected: summary.is_some() || !events.is_empty(),
        summary,
        events,
        next_after,
        has_more,
    }))
}

pub(in crate::http) async fn cancel_job(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<CancelJobResponse>, ApiError> {
    const CANCEL_MESSAGE: &str = "Optimization cancelled at the user's request.";

    let started = Instant::now();
    let pair_id = state
        .observation
        .as_ref()
        .map(|observation| observation.next_pair_id("cancel"));
    let request_entry = match (&state.observation, &pair_id) {
        (Some(observation), Some(pair_id)) => {
            let mut entry = observation.entry(
                pair_id,
                ObservationDirection::Request,
                ObservationPeer::Testbed,
                ObservationPeer::Troute,
            );
            entry.method = Some("POST".to_owned());
            entry.path = Some(format!("/integration/jobs/{job_id}/cancel"));
            entry.headers = allowlisted_headers(&headers);
            Some(entry)
        }
        _ => None,
    };

    let store = state.job_store.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "Job not found.",
            &job_id,
        )
    })?;
    // Hold a clone across the persisted transition so optimize cleanup cannot
    // remove the token in the result-vs-cancel race.
    let active = state
        .runner
        .as_ref()
        .and_then(|runner| runner.activity(&job_id));

    match store.cancel_job(&job_id, CANCEL_MESSAGE) {
        Ok(job) => {
            if let Some(active) = active {
                active.publish_persisted(store.as_ref(), &job_id, JobEventKind::Cancelled);
                active.cancel();
            }
            let response = CancelJobResponse {
                job_id: job_id.clone(),
                status: job.status,
            };
            record_cancel_exchange(
                state.observation.as_ref(),
                &job_id,
                pair_id.as_deref(),
                request_entry,
                started,
                StatusCode::OK,
                serde_json::to_value(&response).ok(),
            );
            Ok(Json(response))
        }
        Err(JobStoreError::JobNotFound(_)) => Err(ApiError::new(
            StatusCode::NOT_FOUND,
            "JOB_NOT_FOUND",
            "Job not found.",
            &job_id,
        )),
        Err(JobStoreError::JobNotCancellable { status, .. }) => {
            let error = ApiError::new(
                StatusCode::CONFLICT,
                "JOB_NOT_CANCELLABLE",
                "The job is already terminal.",
                format!("job status is {}", status.as_str()),
            )
            .with_detail(status.as_str());
            let body = json!({
                "error": {
                    "code": error.code,
                    "message": error.message,
                    "detail": error.detail,
                }
            });
            record_cancel_exchange(
                state.observation.as_ref(),
                &job_id,
                pair_id.as_deref(),
                request_entry,
                started,
                StatusCode::CONFLICT,
                Some(body),
            );
            Err(error)
        }
        Err(error) => Err(ApiError::from_storage(error)),
    }
}

pub(in crate::http) async fn job_timeline(
    State(state): State<AppState>,
    Path(job_id): Path<String>,
) -> Json<JobTimelineResponse> {
    let mut indexed_entries = state
        .observation
        .as_ref()
        .map(|observation| observation.list(&job_id))
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .collect::<Vec<_>>();
    indexed_entries.sort_by(|(left_index, left), (right_index, right)| {
        left.timestamp_ms
            .cmp(&right.timestamp_ms)
            .then_with(|| left_index.cmp(right_index))
    });
    let entries = indexed_entries
        .into_iter()
        .map(|(_, entry)| entry)
        .collect();

    Json(JobTimelineResponse { job_id, entries })
}

fn record_job_inspection(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    headers: &HeaderMap,
    started: Instant,
    job: &StoredJob,
) {
    let Some(observation) = observation else {
        return;
    };
    let pair_id = observation.next_pair_id("inspect");
    let mut request = observation.entry(
        &pair_id,
        ObservationDirection::Request,
        ObservationPeer::Testbed,
        ObservationPeer::Troute,
    );
    request.method = Some("GET".to_owned());
    request.path = Some(format!("/integration/jobs/{job_id}"));
    request.headers = allowlisted_headers(headers);
    observation.append(job_id, request);

    let mut response = observation.entry(
        &pair_id,
        ObservationDirection::Response,
        ObservationPeer::Troute,
        ObservationPeer::Testbed,
    );
    response.status = Some(StatusCode::OK.as_u16());
    response.latency_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
    response.headers = Some(ObservationHeaders::from([(
        header::CONTENT_TYPE.as_str().to_owned(),
        "application/json".to_owned(),
    )]));
    response.body = serde_json::to_value(job).ok();
    observation.append(job_id, response);
}

#[allow(clippy::too_many_arguments)]
fn record_cancel_exchange(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    pair_id: Option<&str>,
    request: Option<JobTimelineEntry>,
    started: Instant,
    status: StatusCode,
    body: Option<serde_json::Value>,
) {
    let (Some(observation), Some(pair_id), Some(request)) = (observation, pair_id, request) else {
        return;
    };
    observation.append(job_id, request);
    let mut response = observation.entry(
        pair_id,
        ObservationDirection::Response,
        ObservationPeer::Troute,
        ObservationPeer::Testbed,
    );
    response.status = Some(status.as_u16());
    response.latency_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
    response.headers = Some(ObservationHeaders::from([(
        header::CONTENT_TYPE.as_str().to_owned(),
        "application/json".to_owned(),
    )]));
    response.body = body;
    observation.append(job_id, response);
}

fn record_submit_response(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    pair_id: Option<&str>,
    started: Instant,
    body: &SubmitJobResponse,
) {
    let (Some(observation), Some(pair_id)) = (observation, pair_id) else {
        return;
    };
    let mut entry = observation.entry(
        pair_id,
        ObservationDirection::Response,
        ObservationPeer::Troute,
        ObservationPeer::Testbed,
    );
    entry.status = Some(StatusCode::ACCEPTED.as_u16());
    entry.latency_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
    entry.headers = Some(ObservationHeaders::from([(
        header::CONTENT_TYPE.as_str().to_owned(),
        "application/json".to_owned(),
    )]));
    entry.body = serde_json::to_value(body).ok();
    observation.append(job_id, entry);
}
