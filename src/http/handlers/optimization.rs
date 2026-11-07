use super::super::*;

pub(in crate::http) async fn optimize(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Result<Json<OptimizeRouteRequest>, JsonRejection>,
) -> Result<Json<OptimizeRouteResponse>, ApiError> {
    let Json(mut request) = request.map_err(ApiError::from_json_rejection)?;
    state.executor.apply_request_defaults(&mut request);
    let context = OptimizationContext::prepare(&state, &headers, request)?;
    let outcome = context.execute().await?;
    let response = outcome.into_response(&context.request);

    context.record_response(&response);
    response.map(Json)
}

struct OptimizationContext<'a> {
    state: &'a AppState,
    headers: &'a HeaderMap,
    request: OptimizeRouteRequest,
    job_id: String,
    started: Instant,
    pair_id: Option<String>,
}

impl<'a> OptimizationContext<'a> {
    fn prepare(
        state: &'a AppState,
        headers: &'a HeaderMap,
        request: OptimizeRouteRequest,
    ) -> Result<Self, ApiError> {
        OptimizationProblem::try_from(request.clone())
            .map_err(OptimizationServiceError::InvalidRequest)
            .map_err(ApiError::from_service)?;
        enforce_portfolio_experiment(state, &request)?;
        enforce_routing_pair_limit(&request, state.max_routing_pairs)?;

        let job_id = request.job_id.clone();
        let started = Instant::now();
        let pair_id = state
            .observation
            .as_ref()
            .map(|observation| observation.next_pair_id("opt"));
        Ok(Self {
            state,
            headers,
            request,
            job_id,
            started,
            pair_id,
        })
    }

    async fn execute(&self) -> Result<OptimizationOutcome, ApiError> {
        match &self.state.runner {
            Some(runner) => self.execute_persisted(runner).await,
            None => self.execute_direct().await,
        }
    }

    async fn execute_persisted(&self, runner: &JobRunner) -> Result<OptimizationOutcome, ApiError> {
        let submission = runner
            .submit_with_completion(self.request.clone())
            .map_err(ApiError::from_storage)?;
        self.record_request();
        let completion = submission
            .completion
            .await
            .map_err(|_| {
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "JOB_EXECUTION_FAILED",
                    "Route optimization failed unexpectedly.",
                    "background job ended without reporting completion",
                )
            })?
            .map_err(ApiError::from_storage)?;

        Ok(OptimizationOutcome {
            result: completion.result,
            persisted_error: completion.persisted_error,
            analyzed_suggestions: None,
        })
    }

    async fn execute_direct(&self) -> Result<OptimizationOutcome, ApiError> {
        self.record_request();
        let executor = self.state.executor.clone();
        let cancellation = CancellationToken::new();
        let execution_request = self.request.clone();
        let analysis_request = self.request.clone();
        let (result, suggestions) = tokio::task::spawn_blocking(move || {
            let result = executor.execute(
                execution_request,
                &NoopOptimizationEventReporter,
                &cancellation,
            );
            let suggestions = result
                .as_ref()
                .err()
                .map(|error| executor.failure_suggestions(&analysis_request, error, &cancellation))
                .unwrap_or_default();
            (result, suggestions)
        })
        .await
        .map_err(|error| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "JOB_EXECUTION_FAILED",
                "Route optimization failed unexpectedly.",
                error.to_string(),
            )
        })?;

        Ok(OptimizationOutcome {
            result,
            persisted_error: None,
            analyzed_suggestions: Some(suggestions),
        })
    }

    fn record_request(&self) {
        record_optimize_request(
            self.state.observation.as_ref(),
            &self.job_id,
            self.pair_id.as_deref(),
            self.headers,
            &self.request,
        );
    }

    fn record_response(&self, response: &Result<OptimizeRouteResponse, ApiError>) {
        let (status, body) = match response {
            Ok(response) => (StatusCode::OK, serde_json::to_value(response).ok()),
            Err(error) => (
                error.status,
                serde_json::to_value(ErrorEnvelope {
                    error: error.body(),
                })
                .ok(),
            ),
        };
        record_optimize_response(
            self.state.observation.as_ref(),
            &self.job_id,
            self.pair_id.as_deref(),
            self.started,
            status,
            body,
        );
    }
}

struct OptimizationOutcome {
    result: Result<OptimizeRouteResponse, OptimizationServiceError>,
    persisted_error: Option<crate::storage::StoredJobError>,
    analyzed_suggestions: Option<Vec<FailureSuggestion>>,
}

impl OptimizationOutcome {
    fn into_response(
        self,
        request: &OptimizeRouteRequest,
    ) -> Result<OptimizeRouteResponse, ApiError> {
        match self.result {
            Ok(mut response) => {
                if request.portfolio_experiment.is_none() {
                    response.strip_portfolio_extensions();
                }
                Ok(response)
            }
            Err(error) => Err(optimization_api_error(
                error,
                request,
                self.persisted_error,
                self.analyzed_suggestions,
            )),
        }
    }
}

fn optimization_api_error(
    error: OptimizationServiceError,
    request: &OptimizeRouteRequest,
    persisted_error: Option<crate::storage::StoredJobError>,
    analyzed_suggestions: Option<Vec<FailureSuggestion>>,
) -> ApiError {
    let mut error = ApiError::from_service_with_request(error, Some(request));
    if let Some(suggestions) = analyzed_suggestions {
        error.suggestions = suggestions;
    }
    if let Some(persisted) = persisted_error {
        match persisted.code.as_str() {
            "TIME_WINDOW_VIOLATION" => {
                error.code = "TIME_WINDOW_VIOLATION";
                error.message = "A location cannot be visited within its opening window.";
            }
            "START_POLICY_INFEASIBLE" => {
                error.code = "START_POLICY_INFEASIBLE";
                error.message = "The selected start policy cannot produce a feasible route.";
            }
            "ROUTING_PAIR_FAILED" => {
                error.code = "ROUTING_PAIR_FAILED";
                error.message = "A route could not be found for one location pair.";
            }
            _ => {}
        }
        error.detail = Some(persisted.detail);
        if let Some(failure_detail) = persisted.failure_detail {
            error.failure_detail = Some(Box::new(failure_detail));
        }
        if let Some(suggestions) = persisted.suggestions {
            error.suggestions = suggestions;
        }
    }
    error
}

fn record_optimize_request(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    pair_id: Option<&str>,
    headers: &HeaderMap,
    request: &OptimizeRouteRequest,
) {
    let (Some(observation), Some(pair_id)) = (observation, pair_id) else {
        return;
    };
    let mut entry = observation.entry(
        pair_id,
        ObservationDirection::Request,
        ObservationPeer::Testbed,
        ObservationPeer::Troute,
    );
    entry.method = Some("POST".to_owned());
    entry.path = Some("/optimize".to_owned());
    entry.headers = allowlisted_headers(headers);
    entry.body = serde_json::to_value(request).ok();
    observation.append(job_id, entry);
}

fn record_optimize_response(
    observation: Option<&JobObservationRecorder>,
    job_id: &str,
    pair_id: Option<&str>,
    started: Instant,
    status: StatusCode,
    body: Option<serde_json::Value>,
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
    entry.status = Some(status.as_u16());
    entry.latency_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
    entry.headers = Some(ObservationHeaders::from([(
        header::CONTENT_TYPE.as_str().to_owned(),
        "application/json".to_owned(),
    )]));
    entry.body = body;
    observation.append(job_id, entry);
}
