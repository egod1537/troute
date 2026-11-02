use super::*;

#[test]
fn progress_stages_and_error_codes_have_stable_wire_values() {
    let stages = [
        (ProgressStage::Accepted, "accepted"),
        (ProgressStage::ValidatingRequest, "validating_request"),
        (ProgressStage::SelectingProvider, "selecting_provider"),
        (ProgressStage::PreparingMatrix, "preparing_matrix"),
        (ProgressStage::FetchingTravelTimes, "fetching_travel_times"),
        (ProgressStage::BuildingMatrix, "building_matrix"),
        (ProgressStage::GeneratingCandidates, "generating_candidates"),
        (ProgressStage::OptimizingRoute, "optimizing_route"),
        (
            ProgressStage::SelectingBestCandidate,
            "selecting_best_candidate",
        ),
        (ProgressStage::Scheduling, "scheduling"),
        (ProgressStage::ValidatingSchedule, "validating_schedule"),
        (ProgressStage::FinalizingResult, "finalizing_result"),
    ];
    for (stage, wire) in stages {
        assert_eq!(serde_json::to_value(stage).unwrap(), wire);
    }
    assert_eq!(
        serde_json::from_str::<ProgressStage>(r#""solving""#).unwrap(),
        ProgressStage::OptimizingRoute
    );
    assert_eq!(
        serde_json::to_value(OptimizationErrorCode::NoFeasibleRoute).unwrap(),
        "NO_FEASIBLE_ROUTE"
    );
}

#[test]
fn progress_ranges_are_ordered_and_bounded() {
    let stages = [
        ProgressStage::Accepted,
        ProgressStage::ValidatingRequest,
        ProgressStage::SelectingProvider,
        ProgressStage::PreparingMatrix,
        ProgressStage::FetchingTravelTimes,
        ProgressStage::BuildingMatrix,
        ProgressStage::GeneratingCandidates,
        ProgressStage::OptimizingRoute,
        ProgressStage::SelectingBestCandidate,
        ProgressStage::Scheduling,
        ProgressStage::ValidatingSchedule,
        ProgressStage::FinalizingResult,
    ];
    assert!(stages.windows(2).all(|pair| {
        pair[0].range().start <= pair[0].range().end && pair[0].range().end <= pair[1].range().start
    }));
    assert!(stages.last().unwrap().range().end < 100);
}
