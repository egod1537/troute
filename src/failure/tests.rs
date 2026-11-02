use super::*;

fn request(value: serde_json::Value) -> OptimizeRouteRequest {
    serde_json::from_value(value).unwrap()
}

#[test]
fn impossible_stay_and_late_fixed_start_produce_quantified_suggestions() {
    let request = request(serde_json::json!({
        "job_id": "failure-suggestions",
        "start_policy": "FIXED",
        "start_time": "18:00",
        "locations": [
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"museum","place_id":"b","open_time":"10:00","close_time":"10:30","stay_minutes":60},
            {"id":"D","place_id":"d","open_time":"00:00","close_time":"19:00","stay_minutes":0}
        ],
        "travel_time_matrix": [[0,20,20],[20,0,20],[20,20,0]]
    }));

    let suggestions = no_feasible_suggestions(&request);
    let reduce = suggestions
        .iter()
        .find(|item| item.suggestion_type == FailureSuggestionType::ReduceStayTime)
        .unwrap();
    assert_eq!(reduce.location_id.as_deref(), Some("museum"));
    assert_eq!(reduce.current_minutes, Some(60));
    assert_eq!(reduce.suggested_max_minutes, Some(30));
    assert!(suggestions
        .iter()
        .any(|item| item.suggestion_type == FailureSuggestionType::StartEarlier));
    assert!(suggestions
        .iter()
        .any(|item| item.suggestion_type == FailureSuggestionType::ChangeStartPolicy));
}

#[test]
fn all_suggestion_types_have_stable_wire_values() {
    let values = [
        (FailureSuggestionType::ReduceStayTime, "REDUCE_STAY_TIME"),
        (
            FailureSuggestionType::MoveLocationEarlier,
            "MOVE_LOCATION_EARLIER",
        ),
        (
            FailureSuggestionType::MoveLocationLater,
            "MOVE_LOCATION_LATER",
        ),
        (FailureSuggestionType::StartEarlier, "START_EARLIER"),
        (FailureSuggestionType::StartLater, "START_LATER"),
        (
            FailureSuggestionType::ChangeStartPolicy,
            "CHANGE_START_POLICY",
        ),
        (
            FailureSuggestionType::ChangeStartLocation,
            "CHANGE_START_LOCATION",
        ),
        (
            FailureSuggestionType::ChangeEndLocation,
            "CHANGE_END_LOCATION",
        ),
        (FailureSuggestionType::RemoveLocation, "REMOVE_LOCATION"),
        (
            FailureSuggestionType::ChangeTravelMode,
            "CHANGE_TRAVEL_MODE",
        ),
        (FailureSuggestionType::SplitDay, "SPLIT_DAY"),
        (FailureSuggestionType::RetryRouting, "RETRY_ROUTING"),
    ];
    for (value, expected) in values {
        assert_eq!(serde_json::to_value(value).unwrap(), expected);
    }
}

#[test]
fn fixed_destination_suggests_an_exact_earlier_start_not_reordering() {
    let error = ScheduleError::TimeWindowViolation {
        location_id: "museum".to_owned(),
        arrival_time: TimeOfDay::from_minutes(9 * 60 + 15).unwrap(),
        service_start_time: TimeOfDay::from_minutes(9 * 60 + 15).unwrap(),
        required_departure: TimeOfDay::from_minutes(9 * 60 + 45).unwrap(),
        close_time: TimeOfDay::from_minutes(9 * 60 + 10).unwrap(),
        selected_start_time: TimeOfDay::from_minutes(9 * 60).unwrap(),
        total_wait_minutes_before_violation: 0,
        is_start: false,
        is_destination: true,
        current_stay_minutes: 30,
        suggested_max_stay_minutes: 0,
    };

    let suggestions = schedule_suggestions(&error);
    assert_eq!(suggestions.len(), 1);
    assert_eq!(
        suggestions[0].suggestion_type,
        FailureSuggestionType::StartEarlier
    );
    assert_eq!(
        suggestions[0].suggested_latest_start.unwrap().to_string(),
        "08:25"
    );
}

#[test]
fn configuration_failures_do_not_offer_an_unfounded_retry() {
    let error = RoutingError::ProviderNotConfigured {
        provider: "otp".to_owned(),
        reason: "OTP_BASE_URL is missing".to_owned(),
    };

    assert!(routing_suggestions(&error).is_empty());
}

#[test]
fn time_window_overrun_reports_minimum_shift_and_maximum_stay() {
    let error = ScheduleError::TimeWindowViolation {
        location_id: "gallery".to_owned(),
        arrival_time: TimeOfDay::from_minutes(9 * 60).unwrap(),
        service_start_time: TimeOfDay::from_minutes(9 * 60).unwrap(),
        required_departure: TimeOfDay::from_minutes(9 * 60 + 30).unwrap(),
        close_time: TimeOfDay::from_minutes(9 * 60 + 20).unwrap(),
        selected_start_time: TimeOfDay::from_minutes(8 * 60 + 30).unwrap(),
        total_wait_minutes_before_violation: 0,
        is_start: false,
        is_destination: false,
        current_stay_minutes: 30,
        suggested_max_stay_minutes: 20,
    };

    let suggestions = schedule_suggestions(&error);
    let reduce = suggestions
        .iter()
        .find(|item| item.suggestion_type == FailureSuggestionType::ReduceStayTime)
        .unwrap();
    assert_eq!(reduce.current_minutes, Some(30));
    assert_eq!(reduce.suggested_max_minutes, Some(20));
    let move_earlier = suggestions
        .iter()
        .find(|item| item.suggestion_type == FailureSuggestionType::MoveLocationEarlier)
        .unwrap();
    assert_eq!(move_earlier.required_shift_minutes, Some(10));
    assert_eq!(move_earlier.suggested_value, Some(json!("08:50")));
    assert!(!suggestions
        .iter()
        .any(|item| item.suggestion_type == FailureSuggestionType::StartEarlier));
}

#[test]
fn start_window_violation_suggests_stay_reduction_not_reordering() {
    let error = ScheduleError::TimeWindowViolation {
        location_id: "start".to_owned(),
        arrival_time: TimeOfDay::from_minutes(9 * 60).unwrap(),
        service_start_time: TimeOfDay::from_minutes(9 * 60).unwrap(),
        required_departure: TimeOfDay::from_minutes(10 * 60).unwrap(),
        close_time: TimeOfDay::from_minutes(9 * 60 + 30).unwrap(),
        selected_start_time: TimeOfDay::from_minutes(9 * 60).unwrap(),
        total_wait_minutes_before_violation: 0,
        is_start: true,
        is_destination: false,
        current_stay_minutes: 60,
        suggested_max_stay_minutes: 30,
    };

    let suggestions = schedule_suggestions(&error);
    assert!(suggestions
        .iter()
        .any(|item| item.suggestion_type == FailureSuggestionType::ReduceStayTime));
    assert!(!suggestions
        .iter()
        .any(|item| item.suggestion_type == FailureSuggestionType::MoveLocationEarlier));
}

#[test]
fn late_latest_lower_bound_suggests_earliest_policy_and_earlier_start() {
    let request = request(serde_json::json!({
        "job_id": "latest-remediation",
        "start_policy": "LATEST",
        "start_time": "18:00",
        "locations": [
            {"id":"A","place_id":"a","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"D","place_id":"d","open_time":"00:00","close_time":"18:30","stay_minutes":30}
        ],
        "travel_time_matrix": [[0,30],[30,0]]
    }));

    let suggestions = no_feasible_suggestions(&request);
    assert!(suggestions
        .iter()
        .any(|item| item.suggestion_type == FailureSuggestionType::StartEarlier));
    let policy = suggestions
        .iter()
        .find(|item| item.suggestion_type == FailureSuggestionType::ChangeStartPolicy)
        .unwrap();
    assert_eq!(policy.current_value, Some(json!("LATEST")));
    assert_eq!(policy.suggested_value, Some(json!("EARLIEST")));
}

#[test]
fn ranking_prefers_small_changes_and_limits_the_result() {
    let suggestions = vec![
        FailureSuggestion::new(
            FailureSuggestionType::SplitDay,
            SuggestionConfidence::Heuristic,
            "split",
        ),
        FailureSuggestion::new(
            FailureSuggestionType::RemoveLocation,
            SuggestionConfidence::Heuristic,
            "remove",
        ),
        FailureSuggestion::new(
            FailureSuggestionType::MoveLocationEarlier,
            SuggestionConfidence::Exact,
            "move",
        ),
        FailureSuggestion::new(
            FailureSuggestionType::ReduceStayTime,
            SuggestionConfidence::Exact,
            "reduce",
        ),
        FailureSuggestion::new(
            FailureSuggestionType::StartEarlier,
            SuggestionConfidence::Exact,
            "start",
        ),
    ];

    let ranked = rank_failure_suggestions(suggestions, 3);
    assert_eq!(ranked.len(), 3);
    assert_eq!(
        ranked[0].suggestion_type,
        FailureSuggestionType::StartEarlier
    );
    assert_eq!(
        ranked[1].suggestion_type,
        FailureSuggestionType::ReduceStayTime
    );
    assert_eq!(
        ranked[2].suggestion_type,
        FailureSuggestionType::MoveLocationEarlier
    );
}

#[test]
fn ranking_deduplicates_conflicts_using_the_smallest_sufficient_change() {
    let start = |minutes: u32, reason: &str| {
        let mut suggestion = FailureSuggestion::new(
            FailureSuggestionType::StartEarlier,
            SuggestionConfidence::Exact,
            reason,
        );
        suggestion.required_shift_minutes = Some(minutes);
        suggestion
    };
    let mut later = FailureSuggestion::new(
        FailureSuggestionType::StartLater,
        SuggestionConfidence::Heuristic,
        "conflicting later change",
    );
    later.required_shift_minutes = Some(5);

    let ranked = rank_failure_suggestions(vec![start(40, "forty"), later, start(30, "thirty")], 3);
    assert_eq!(ranked.len(), 1);
    assert_eq!(
        ranked[0].suggestion_type,
        FailureSuggestionType::StartEarlier
    );
    assert_eq!(ranked[0].required_shift_minutes, Some(30));
}

#[test]
fn ranking_is_stable_for_equivalent_mode_candidates_and_honors_zero_limit() {
    let candidates = vec![
        change_travel_mode_suggestion(TravelMode::Transit, TravelMode::Walking, None),
        change_travel_mode_suggestion(TravelMode::Transit, TravelMode::Driving, None),
    ];
    let forward = rank_failure_suggestions(candidates.clone(), 3);
    let reverse = rank_failure_suggestions(candidates.into_iter().rev().collect(), 3);

    assert_eq!(forward, reverse);
    assert_eq!(forward.len(), 1);
    assert_eq!(forward[0].suggested_value, Some(json!("DRIVING")));
    assert!(rank_failure_suggestions(forward, 0).is_empty());
}
