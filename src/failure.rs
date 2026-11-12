use std::{collections::HashSet, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    api::OptimizeRouteRequest,
    domain::{StartPolicy, TimeOfDay},
    routing::{RoutingError, TravelMode},
    schedule::ScheduleError,
    service::OptimizationServiceError,
    solver::SolverError,
};

pub const DEFAULT_REMEDIATION_BUDGET_MS: u64 = 500;
pub const DEFAULT_REMEDIATION_MAX_REMOVE_PROBES: usize = 8;
pub const DEFAULT_MAX_FAILURE_SUGGESTIONS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemediationConfig {
    pub budget: Duration,
    pub max_remove_probes: usize,
    pub max_suggestions: usize,
}

impl Default for RemediationConfig {
    fn default() -> Self {
        Self {
            budget: Duration::from_millis(DEFAULT_REMEDIATION_BUDGET_MS),
            max_remove_probes: DEFAULT_REMEDIATION_MAX_REMOVE_PROBES,
            max_suggestions: DEFAULT_MAX_FAILURE_SUGGESTIONS,
        }
    }
}

/// Stable machine-readable categories that clients can map to UI actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureSuggestionType {
    ReduceStayTime,
    MoveLocationEarlier,
    MoveLocationLater,
    StartEarlier,
    StartLater,
    ChangeStartPolicy,
    ChangeStartLocation,
    ChangeEndLocation,
    RemoveLocation,
    ChangeTravelMode,
    SplitDay,
    RetryRouting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SuggestionConfidence {
    Exact,
    Heuristic,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FailureSuggestion {
    #[serde(rename = "type")]
    pub suggestion_type: FailureSuggestionType,
    pub reason: String,
    pub confidence: SuggestionConfidence,
    /// True only after validating a complete itinerary with this single change.
    #[serde(default, skip_serializing_if = "is_false")]
    pub feasibility_verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_location_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_location_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_max_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_shift_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_latest_start: Option<TimeOfDay>,
}

impl FailureSuggestion {
    pub(crate) fn new(
        suggestion_type: FailureSuggestionType,
        confidence: SuggestionConfidence,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            suggestion_type,
            reason: reason.into(),
            confidence,
            feasibility_verified: false,
            location_id: None,
            from_location_id: None,
            to_location_id: None,
            current_value: None,
            suggested_value: None,
            current_minutes: None,
            suggested_max_minutes: None,
            required_shift_minutes: None,
            suggested_latest_start: None,
        }
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

/// Applies the stable user-disruption ranking, removes equivalent or
/// conflicting actions, and limits the response size.
pub fn rank_failure_suggestions(
    suggestions: Vec<FailureSuggestion>,
    maximum: usize,
) -> Vec<FailureSuggestion> {
    if maximum == 0 {
        return Vec::new();
    }
    let mut ranked = suggestions.into_iter().enumerate().collect::<Vec<_>>();
    ranked.sort_by_key(|(index, suggestion)| ranking_key(suggestion, *index));

    let mut conflicts = HashSet::new();
    let mut result = Vec::with_capacity(maximum.min(ranked.len()));
    for (_, suggestion) in ranked {
        if conflicts.insert(conflict_key(&suggestion)) {
            result.push(suggestion);
            if result.len() == maximum {
                break;
            }
        }
    }
    result
}

fn ranking_key(
    suggestion: &FailureSuggestion,
    original_index: usize,
) -> (u64, u8, String, String, String, usize) {
    let type_priority = match suggestion.suggestion_type {
        FailureSuggestionType::StartEarlier | FailureSuggestionType::StartLater => 1_u64,
        FailureSuggestionType::ReduceStayTime => 2,
        FailureSuggestionType::MoveLocationEarlier | FailureSuggestionType::MoveLocationLater => 3,
        FailureSuggestionType::ChangeStartPolicy => 4,
        FailureSuggestionType::ChangeTravelMode | FailureSuggestionType::RetryRouting => 5,
        FailureSuggestionType::ChangeStartLocation | FailureSuggestionType::ChangeEndLocation => 6,
        FailureSuggestionType::RemoveLocation => 7,
        FailureSuggestionType::SplitDay => 8,
    };
    let confidence_penalty = match suggestion.confidence {
        SuggestionConfidence::Exact => 0_u64,
        SuggestionConfidence::Heuristic => 20_000,
    };
    let feasibility_penalty = match suggestion.suggestion_type {
        FailureSuggestionType::ChangeStartPolicy => 5_000,
        FailureSuggestionType::RetryRouting => 8_000,
        // These heuristic suggestions are emitted only after successful probes.
        FailureSuggestionType::ChangeTravelMode
        | FailureSuggestionType::RemoveLocation
        | FailureSuggestionType::SplitDay => 0,
        _ => 2_000,
    };
    let routing_penalty = match suggestion.suggestion_type {
        FailureSuggestionType::ChangeTravelMode => 1_000,
        FailureSuggestionType::RetryRouting => 3_000,
        _ => 0,
    };
    let magnitude = u64::from(
        change_magnitude_minutes(suggestion)
            .unwrap_or(9_999)
            .min(9_999),
    );
    let score = type_priority * 100_000
        + confidence_penalty
        + feasibility_penalty
        + routing_penalty
        + magnitude;
    (
        score,
        suggestion_type_order(suggestion.suggestion_type),
        target_key(suggestion),
        suggestion
            .suggested_value
            .as_ref()
            .map(Value::to_string)
            .unwrap_or_default(),
        suggestion.reason.clone(),
        original_index,
    )
}

fn suggestion_type_order(suggestion_type: FailureSuggestionType) -> u8 {
    match suggestion_type {
        FailureSuggestionType::StartEarlier => 0,
        FailureSuggestionType::StartLater => 1,
        FailureSuggestionType::ReduceStayTime => 2,
        FailureSuggestionType::MoveLocationEarlier => 3,
        FailureSuggestionType::MoveLocationLater => 4,
        FailureSuggestionType::ChangeStartPolicy => 5,
        FailureSuggestionType::ChangeTravelMode => 6,
        FailureSuggestionType::RetryRouting => 7,
        FailureSuggestionType::ChangeStartLocation => 8,
        FailureSuggestionType::ChangeEndLocation => 9,
        FailureSuggestionType::RemoveLocation => 10,
        FailureSuggestionType::SplitDay => 11,
    }
}

fn change_magnitude_minutes(suggestion: &FailureSuggestion) -> Option<u32> {
    suggestion
        .required_shift_minutes
        .or_else(|| {
            suggestion
                .current_minutes
                .zip(suggestion.suggested_max_minutes)
                .map(|(current, suggested)| current.abs_diff(suggested))
        })
        .or_else(|| {
            suggestion
                .current_value
                .as_ref()
                .and_then(Value::as_u64)
                .zip(suggestion.suggested_value.as_ref().and_then(Value::as_u64))
                .and_then(|(current, suggested)| u32::try_from(current.abs_diff(suggested)).ok())
        })
        .or_else(|| {
            let current = suggestion.current_value.as_ref()?.as_str()?;
            let suggested = suggestion.suggested_value.as_ref()?.as_str()?;
            Some(parse_time_minutes(current)?.abs_diff(parse_time_minutes(suggested)?))
        })
}

fn parse_time_minutes(value: &str) -> Option<u32> {
    let (hour, minute) = value.split_once(':')?;
    let hour = hour.parse::<u32>().ok()?;
    let minute = minute.parse::<u32>().ok()?;
    (hour < 24 && minute < 60).then_some(hour * 60 + minute)
}

fn target_key(suggestion: &FailureSuggestion) -> String {
    format!(
        "{}|{}|{}",
        suggestion.location_id.as_deref().unwrap_or_default(),
        suggestion.from_location_id.as_deref().unwrap_or_default(),
        suggestion.to_location_id.as_deref().unwrap_or_default()
    )
}

fn conflict_key(suggestion: &FailureSuggestion) -> String {
    match suggestion.suggestion_type {
        FailureSuggestionType::StartEarlier | FailureSuggestionType::StartLater => {
            "start-time".to_owned()
        }
        FailureSuggestionType::MoveLocationEarlier | FailureSuggestionType::MoveLocationLater => {
            format!(
                "move:{}",
                suggestion.location_id.as_deref().unwrap_or_default()
            )
        }
        FailureSuggestionType::ReduceStayTime => format!(
            "stay:{}",
            suggestion.location_id.as_deref().unwrap_or_default()
        ),
        FailureSuggestionType::ChangeStartPolicy => "start-policy".to_owned(),
        FailureSuggestionType::ChangeTravelMode => "travel-mode".to_owned(),
        FailureSuggestionType::ChangeStartLocation => "start-location".to_owned(),
        FailureSuggestionType::ChangeEndLocation => "end-location".to_owned(),
        FailureSuggestionType::RemoveLocation => format!(
            "remove:{}",
            suggestion.location_id.as_deref().unwrap_or_default()
        ),
        FailureSuggestionType::SplitDay => "split-day".to_owned(),
        FailureSuggestionType::RetryRouting => "retry-routing".to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FailureDetail {
    InvalidRequest,
    RoutingUnavailable,
    RoutingPairFailed {
        from_location_id: String,
        to_location_id: String,
    },
    ProviderResolution,
    ProviderNotConfigured,
    UnsupportedProviderCapability,
    NoFeasibleRoute,
    StartPolicyInfeasible {
        start_policy: StartPolicy,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_start_time: Option<TimeOfDay>,
    },
    TimeWindowViolation {
        location_id: String,
        arrival_time: TimeOfDay,
        service_start_time: TimeOfDay,
        required_departure: TimeOfDay,
        close_time: TimeOfDay,
        stay_minutes: u32,
    },
    OutsideSingleDay,
    Cancelled,
    Internal,
}

/// Produces only necessary-condition suggestions that can be established from
/// the request itself. It never claims that applying one change guarantees a
/// feasible route.
pub(crate) fn no_feasible_suggestions(request: &OptimizeRouteRequest) -> Vec<FailureSuggestion> {
    let mut suggestions = Vec::new();

    // A stay longer than its entire opening window is locally impossible,
    // independent of visit order or travel times.
    for location in &request.locations {
        let window_minutes = u32::from(
            location
                .close_time
                .minutes()
                .saturating_sub(location.open_time.minutes()),
        );
        if location.stay_minutes > window_minutes {
            let mut suggestion = FailureSuggestion::new(
                FailureSuggestionType::ReduceStayTime,
                SuggestionConfidence::Exact,
                "The requested stay is longer than the location's complete opening window.",
            );
            suggestion.location_id = Some(location.id.clone());
            suggestion.current_value = Some(json!(location.stay_minutes));
            suggestion.suggested_value = Some(json!(window_minutes));
            suggestion.current_minutes = Some(location.stay_minutes);
            suggestion.suggested_max_minutes = Some(window_minutes);
            suggestions.push(suggestion);
        }
    }

    let lower_bound = minimum_duration_lower_bound(request);
    let policy = request.start_policy.unwrap_or_default();
    let start = request
        .start_time
        .unwrap_or_else(|| TimeOfDay::from_minutes(0).expect("midnight is valid"));
    let end_close = request
        .locations
        .last()
        .map(|location| u32::from(location.close_time.minutes()))
        .unwrap_or(24 * 60 - 1);

    if lower_bound > end_close {
        let suggested_days = lower_bound.div_ceil(24 * 60).max(2);
        let mut suggestion = FailureSuggestion::new(
            FailureSuggestionType::SplitDay,
            SuggestionConfidence::Heuristic,
            "Even the route-duration lower bound exceeds the available single-day horizon.",
        );
        suggestion.current_value = Some(json!({
            "days": 1,
            "minimum_required_minutes": lower_bound
        }));
        suggestion.suggested_value = Some(json!({ "days": suggested_days }));
        suggestions.push(suggestion);
    }

    let latest_necessary_start = end_close.saturating_sub(lower_bound);
    if u32::from(start.minutes()) > latest_necessary_start {
        let suggested_minutes = latest_necessary_start.min(24 * 60 - 1) as u16;
        let suggested = TimeOfDay::from_minutes(suggested_minutes)
            .expect("the calculated start is inside one day");
        let mut suggestion = FailureSuggestion::new(
            FailureSuggestionType::StartEarlier,
            SuggestionConfidence::Heuristic,
            "The current start is later than the latest start allowed by the route-duration lower bound and destination closing time.",
        );
        suggestion.current_value = Some(json!(start.to_string()));
        suggestion.suggested_value = Some(json!(suggested.to_string()));
        suggestion.required_shift_minutes =
            Some(u32::from(start.minutes()).saturating_sub(u32::from(suggested.minutes())));
        suggestion.suggested_latest_start = Some(suggested);
        suggestions.push(suggestion);

        if matches!(policy, StartPolicy::Fixed | StartPolicy::Latest) {
            let mut policy_suggestion = FailureSuggestion::new(
                FailureSuggestionType::ChangeStartPolicy,
                SuggestionConfidence::Heuristic,
                "Allowing EARLIEST start selection lets the solver search before the current start bound.",
            );
            policy_suggestion.current_value = Some(json!(policy));
            policy_suggestion.suggested_value = Some(json!("EARLIEST"));
            suggestions.push(policy_suggestion);
        }
    }

    suggestions
}

pub(crate) fn routing_suggestions(error: &RoutingError) -> Vec<FailureSuggestion> {
    if matches!(
        error,
        RoutingError::Provider(_) | RoutingError::MatrixBuild { .. }
    ) {
        return vec![FailureSuggestion::new(
            FailureSuggestionType::RetryRouting,
            SuggestionConfidence::Heuristic,
            "Routing failed before optimization; retry after the provider recovers.",
        )];
    }
    Vec::new()
}

pub(crate) fn schedule_suggestions(error: &ScheduleError) -> Vec<FailureSuggestion> {
    let ScheduleError::TimeWindowViolation {
        location_id,
        arrival_time: _,
        service_start_time,
        required_departure,
        close_time,
        selected_start_time,
        total_wait_minutes_before_violation,
        is_start,
        is_destination,
        current_stay_minutes,
        suggested_max_stay_minutes,
    } = error
    else {
        return Vec::new();
    };
    let mut suggestions = Vec::new();

    // Reducing the stay is sufficient for this evaluated arrival only when
    // service itself can begin no later than closing time.
    if service_start_time <= close_time && current_stay_minutes > suggested_max_stay_minutes {
        let mut suggestion = FailureSuggestion::new(
            FailureSuggestionType::ReduceStayTime,
            SuggestionConfidence::Exact,
            "This is the maximum stay that lets the evaluated route finish service by closing time.",
        );
        suggestion.location_id = Some(location_id.clone());
        suggestion.current_value = Some(json!(current_stay_minutes));
        suggestion.suggested_value = Some(json!(suggested_max_stay_minutes));
        suggestion.current_minutes = Some(*current_stay_minutes);
        suggestion.suggested_max_minutes = Some(*suggested_max_stay_minutes);
        suggestions.push(suggestion);
    }

    // Keeping the stay unchanged requires service to start by this exact
    // latest time. This is actionable even when arrival is already past close.
    let required_shift_minutes = u32::from(
        required_departure
            .minutes()
            .saturating_sub(close_time.minutes()),
    );
    let latest_service_start = u32::from(service_start_time.minutes())
        .checked_sub(required_shift_minutes)
        .and_then(|minutes| u16::try_from(minutes).ok())
        .and_then(|minutes| TimeOfDay::from_minutes(minutes).ok());
    if let Some(latest_service_start) = latest_service_start {
        // Both endpoints are fixed by contract, so neither the start nor the
        // destination can be reordered.
        if !*is_start && !*is_destination && service_start_time > &latest_service_start {
            let mut suggestion = FailureSuggestion::new(
                FailureSuggestionType::MoveLocationEarlier,
                SuggestionConfidence::Exact,
                "Keeping the requested stay requires this location's service to start no later than the suggested time.",
            );
            suggestion.location_id = Some(location_id.clone());
            suggestion.current_value = Some(json!(service_start_time.to_string()));
            suggestion.suggested_value = Some(json!(latest_service_start.to_string()));
            suggestion.required_shift_minutes = Some(required_shift_minutes);
            suggestions.push(suggestion);
        }
    }

    if *is_destination {
        let route_shift =
            required_shift_minutes.saturating_add(*total_wait_minutes_before_violation);
        if let Some(suggested_minutes) = u32::from(selected_start_time.minutes())
            .checked_sub(route_shift)
            .and_then(|minutes| u16::try_from(minutes).ok())
        {
            let suggested_start = TimeOfDay::from_minutes(suggested_minutes)
                .expect("calculated start remains within one day");
            let mut suggestion = FailureSuggestion::new(
                FailureSuggestionType::StartEarlier,
                SuggestionConfidence::Exact,
                "Shifting the complete evaluated route earlier by this amount lets the destination service finish by closing time.",
            );
            suggestion.current_value = Some(json!(selected_start_time.to_string()));
            suggestion.suggested_value = Some(json!(suggested_start.to_string()));
            suggestion.required_shift_minutes = Some(route_shift);
            suggestion.suggested_latest_start = Some(suggested_start);
            suggestions.push(suggestion);
        }
    }

    suggestions
}

pub(crate) fn change_travel_mode_suggestion(
    current: TravelMode,
    suggested: TravelMode,
    failed_pair: Option<(&str, &str)>,
) -> FailureSuggestion {
    let mut suggestion = FailureSuggestion::new(
        FailureSuggestionType::ChangeTravelMode,
        SuggestionConfidence::Heuristic,
        "A routing probe completed successfully with this alternative travel mode.",
    );
    suggestion.current_value = Some(json!(current));
    suggestion.suggested_value = Some(json!(suggested));
    if let Some((from, to)) = failed_pair {
        suggestion.from_location_id = Some(from.to_owned());
        suggestion.to_location_id = Some(to.to_owned());
    }
    suggestion
}

pub(crate) fn remove_location_suggestion(location_id: &str) -> FailureSuggestion {
    let mut suggestion = FailureSuggestion::new(
        FailureSuggestionType::RemoveLocation,
        SuggestionConfidence::Heuristic,
        "A bounded feasibility probe found a feasible route after removing this location.",
    );
    suggestion.location_id = Some(location_id.to_owned());
    suggestion.current_value = Some(json!(true));
    suggestion.suggested_value = Some(json!(false));
    suggestion
}

pub(crate) fn split_day_probe_suggestion(location_id: &str) -> FailureSuggestion {
    let mut suggestion = FailureSuggestion::new(
        FailureSuggestionType::SplitDay,
        SuggestionConfidence::Heuristic,
        "A bounded probe found that moving at least one location to another day leaves a feasible single-day route.",
    );
    suggestion.location_id = Some(location_id.to_owned());
    suggestion.current_value = Some(json!({ "days": 1 }));
    suggestion.suggested_value = Some(json!({ "days": 2 }));
    suggestion
}

pub(crate) fn service_failure_detail(
    error: &OptimizationServiceError,
    request: Option<&OptimizeRouteRequest>,
) -> FailureDetail {
    match error {
        OptimizationServiceError::Cancelled => FailureDetail::Cancelled,
        OptimizationServiceError::InvalidRequest(_) => FailureDetail::InvalidRequest,
        OptimizationServiceError::Routing(RoutingError::ProviderResolution(_)) => {
            FailureDetail::ProviderResolution
        }
        OptimizationServiceError::Routing(RoutingError::ProviderNotConfigured { .. }) => {
            FailureDetail::ProviderNotConfigured
        }
        OptimizationServiceError::Routing(RoutingError::UnsupportedProviderCapability {
            ..
        }) => FailureDetail::UnsupportedProviderCapability,
        OptimizationServiceError::Routing(RoutingError::MatrixBuild {
            failed_from,
            failed_to,
            ..
        }) => FailureDetail::RoutingPairFailed {
            from_location_id: failed_from.clone(),
            to_location_id: failed_to.clone(),
        },
        OptimizationServiceError::Routing(_) => FailureDetail::RoutingUnavailable,
        OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => {
            start_policy_failure_detail(request).unwrap_or(FailureDetail::NoFeasibleRoute)
        }
        OptimizationServiceError::Schedule(ScheduleError::TimeWindowViolation {
            location_id,
            arrival_time,
            service_start_time,
            required_departure,
            close_time,
            current_stay_minutes,
            is_destination,
            ..
        }) => {
            if *is_destination {
                if let Some(detail) = start_policy_failure_detail(request) {
                    return detail;
                }
            }
            FailureDetail::TimeWindowViolation {
                location_id: location_id.clone(),
                arrival_time: *arrival_time,
                service_start_time: *service_start_time,
                required_departure: *required_departure,
                close_time: *close_time,
                stay_minutes: *current_stay_minutes,
            }
        }
        OptimizationServiceError::Schedule(ScheduleError::OutsideSingleDay) => {
            FailureDetail::OutsideSingleDay
        }
        OptimizationServiceError::Solver(_) | OptimizationServiceError::Schedule(_) => {
            FailureDetail::Internal
        }
    }
}

fn start_policy_failure_detail(request: Option<&OptimizeRouteRequest>) -> Option<FailureDetail> {
    let request = request?;
    let start_policy = request.start_policy.unwrap_or_default();
    if !matches!(start_policy, StartPolicy::Fixed | StartPolicy::Latest) {
        return None;
    }
    let suggestions = no_feasible_suggestions(request);
    let has_earlier_remediation = suggestions
        .iter()
        .any(|suggestion| suggestion.suggestion_type == FailureSuggestionType::StartEarlier);
    let has_independent_exact_failure = suggestions.iter().any(|suggestion| {
        suggestion.suggestion_type == FailureSuggestionType::ReduceStayTime
            && suggestion.confidence == SuggestionConfidence::Exact
    });
    (has_earlier_remediation && !has_independent_exact_failure).then_some(
        FailureDetail::StartPolicyInfeasible {
            start_policy,
            selected_start_time: request.start_time,
        },
    )
}

pub(crate) fn service_suggestions(
    error: &OptimizationServiceError,
    request: Option<&OptimizeRouteRequest>,
) -> Vec<FailureSuggestion> {
    match error {
        OptimizationServiceError::Routing(source) => routing_suggestions(source),
        OptimizationServiceError::Solver(SolverError::NoFeasibleRoute) => {
            request.map(no_feasible_suggestions).unwrap_or_default()
        }
        OptimizationServiceError::Schedule(source) => {
            let mut suggestions = schedule_suggestions(source);
            if matches!(
                source,
                ScheduleError::OutsideSingleDay | ScheduleError::TimeWindowViolation { .. }
            ) {
                if let Some(request) = request {
                    for suggestion in no_feasible_suggestions(request) {
                        let duplicate = suggestions.iter().any(|existing| {
                            existing.suggestion_type == suggestion.suggestion_type
                                && existing.location_id == suggestion.location_id
                        });
                        if !duplicate {
                            suggestions.push(suggestion);
                        }
                    }
                }
            }
            suggestions
        }
        _ => Vec::new(),
    }
}

fn minimum_duration_lower_bound(request: &OptimizeRouteRequest) -> u32 {
    let stay_minutes = request.locations.iter().fold(0_u32, |total, location| {
        total.saturating_add(location.stay_minutes)
    });
    let matrix: Vec<Vec<Option<u32>>> = if let Some(matrix) = request.travel_time_matrix.as_ref() {
        matrix
            .iter()
            .map(|row| row.iter().copied().map(Some).collect())
            .collect()
    } else if let Some(matrix) = request.sparse_travel_time_matrix.as_ref() {
        matrix.clone()
    } else {
        return stay_minutes;
    };
    let count = matrix.len();
    if count < 2 || matrix.iter().any(|row| row.len() != count) {
        return stay_minutes;
    }

    // Every Hamiltonian start-to-end path needs one outgoing edge from every
    // vertex except the destination and one incoming edge into every vertex
    // except the start. Both sums are valid lower bounds; use the tighter one.
    let outgoing = (0..count.saturating_sub(1)).fold(0_u32, |total, from| {
        let minimum = (0..count)
            .filter(|&to| to != from)
            .filter_map(|to| matrix[from][to])
            .min()
            .unwrap_or(0);
        total.saturating_add(minimum)
    });
    let incoming = (1..count).fold(0_u32, |total, to| {
        let minimum = (0..count)
            .filter(|&from| from != to)
            .filter_map(|from| matrix[from][to])
            .min()
            .unwrap_or(0);
        total.saturating_add(minimum)
    });
    stay_minutes.saturating_add(outgoing.max(incoming))
}

#[cfg(test)]
mod tests;
