use super::*;

#[derive(Debug, Serialize)]
pub(super) struct ErrorEnvelope {
    pub(super) error: ErrorBody,
}

#[derive(Debug, Serialize)]
pub(super) struct ErrorBody {
    code: &'static str,
    message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure_detail: Option<FailureDetail>,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestions: Option<Vec<FailureSuggestion>>,
}

#[derive(Debug)]
pub(super) struct ApiError {
    pub(super) status: StatusCode,
    pub(super) code: &'static str,
    pub(super) message: &'static str,
    pub(super) reason: String,
    pub(super) detail: Option<String>,
    pub(super) failure_detail: Option<Box<FailureDetail>>,
    pub(super) suggestions: Vec<FailureSuggestion>,
}

impl ApiError {
    pub(super) fn new(
        status: StatusCode,
        code: &'static str,
        message: &'static str,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            status,
            code,
            message,
            reason: reason.into(),
            detail: None,
            failure_detail: None,
            suggestions: Vec::new(),
        }
    }

    pub(super) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(super) fn from_json_rejection(rejection: JsonRejection) -> Self {
        let status = rejection.status();
        if status == StatusCode::UNSUPPORTED_MEDIA_TYPE {
            return Self::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "UNSUPPORTED_MEDIA_TYPE",
                "Content-Type must be application/json.",
                rejection.body_text(),
            );
        }
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            return Self::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "REQUEST_TOO_LARGE",
                "Request body must not exceed 1 MiB.",
                rejection.body_text(),
            );
        }
        Self::new(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "Request body must be valid JSON matching the optimize contract.",
            rejection.body_text(),
        )
    }

    pub(super) fn from_service(error: OptimizationServiceError) -> Self {
        Self::from_service_with_request(error, None)
    }

    pub(super) fn from_service_with_request(
        error: OptimizationServiceError,
        request: Option<&OptimizeRouteRequest>,
    ) -> Self {
        let failure_detail = service_failure_detail(&error, request);
        let suggestions = rank_failure_suggestions(
            service_suggestions(&error, request),
            DEFAULT_MAX_FAILURE_SUGGESTIONS,
        );
        let mut api_error = match error {
            OptimizationServiceError::Cancelled => Self::new(
                StatusCode::CONFLICT,
                "JOB_CANCELLED",
                "The optimization job was cancelled.",
                "cancellation was requested before optimization completed",
            ),
            OptimizationServiceError::InvalidRequest(source) => Self::new(
                StatusCode::BAD_REQUEST,
                "INVALID_REQUEST",
                "The optimize request is invalid.",
                source.to_string(),
            ),
            OptimizationServiceError::Routing(source) => match source {
                RoutingError::Provider(reason) => Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ROUTING_UNAVAILABLE",
                    "Travel-time routing is temporarily unavailable.",
                    reason,
                ),
                RoutingError::ProviderResolution(reason) => Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ROUTE_PROVIDER_RESOLUTION_ERROR",
                    "No route provider could be selected.",
                    reason,
                ),
                RoutingError::ProviderOverrideDisabled => Self::new(
                    StatusCode::BAD_REQUEST,
                    "PROVIDER_OVERRIDE_DISABLED",
                    "Request-level route provider override is disabled.",
                    "ROUTE_PROVIDER_OVERRIDE_ENABLED is false",
                ),
                source @ RoutingError::ProviderNotConfigured { .. } => Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "PROVIDER_NOT_CONFIGURED",
                    "The selected route provider is not configured.",
                    source.to_string(),
                ),
                source @ RoutingError::UnsupportedProviderCapability { .. } => Self::new(
                    StatusCode::BAD_REQUEST,
                    "UNSUPPORTED_PROVIDER_CAPABILITY",
                    "The selected route provider does not support this travel mode.",
                    source.to_string(),
                ),
                source @ RoutingError::MatrixBuild { .. } => Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ROUTING_PAIR_FAILED",
                    "A route could not be found for one location pair.",
                    source.to_string(),
                ),
                other => Self::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "ROUTING_MATRIX_ERROR",
                    "Travel-time routing returned an invalid matrix.",
                    other.to_string(),
                ),
            },
            OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => {
                if matches!(&failure_detail, FailureDetail::StartPolicyInfeasible { .. }) {
                    Self::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "START_POLICY_INFEASIBLE",
                        "The selected start policy cannot produce a feasible route.",
                        "the route requires an earlier start",
                    )
                } else {
                    Self::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "NO_FEASIBLE_ROUTE",
                        "No feasible route was found.",
                        "solver found no feasible route",
                    )
                }
            }
            OptimizationServiceError::Schedule(
                source @ (ScheduleError::OutsideSingleDay
                | ScheduleError::TimeWindowViolation { .. }),
            ) => {
                let (code, message) = match &failure_detail {
                    FailureDetail::StartPolicyInfeasible { .. } => (
                        "START_POLICY_INFEASIBLE",
                        "The selected start policy cannot produce a feasible route.",
                    ),
                    FailureDetail::TimeWindowViolation { .. } => (
                        "TIME_WINDOW_VIOLATION",
                        "A location cannot be visited within its opening window.",
                    ),
                    _ => ("NO_FEASIBLE_ROUTE", "No feasible route was found."),
                };
                Self::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    code,
                    message,
                    source.to_string(),
                )
            }
            OptimizationServiceError::Solver(source) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "Route optimization failed unexpectedly.",
                source.to_string(),
            ),
            OptimizationServiceError::Schedule(source) => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "INTERNAL_ERROR",
                "Route scheduling failed unexpectedly.",
                source.to_string(),
            ),
        };
        api_error.detail = Some(api_error.reason.clone());
        api_error.failure_detail = Some(Box::new(failure_detail));
        api_error.suggestions = suggestions;
        api_error
    }

    pub(super) fn from_storage(error: JobStoreError) -> Self {
        match error {
            JobStoreError::DuplicateJob(job_id) => Self::new(
                StatusCode::CONFLICT,
                "DUPLICATE_JOB_ID",
                "A job with this job_id already exists.",
                job_id,
            ),
            JobStoreError::CapacityExceeded { maximum } => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "JOB_CAPACITY_EXCEEDED",
                "The optimization queue is full.",
                format!("maximum_outstanding_jobs={maximum}"),
            ),
            source => Self::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "STORAGE_ERROR",
                "Job records could not be stored.",
                source.to_string(),
            ),
        }
    }

    pub(super) fn body(&self) -> ErrorBody {
        ErrorBody {
            code: self.code,
            message: self.message,
            detail: self.detail.clone(),
            failure_detail: self.failure_detail.as_deref().cloned(),
            suggestions: (!self.suggestions.is_empty()).then(|| self.suggestions.clone()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        logger().http_request_failed(self.code, self.status.as_u16(), &self.reason);
        (self.status, Json(ErrorEnvelope { error: self.body() })).into_response()
    }
}
