fn constrained_problem() -> OptimizationProblem {
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": "sa-infeasible-test",
        "locations": [
            {"id":"0","place_id":"p0","open_time":"00:00","close_time":"23:50","stay_minutes":0},
            {"id":"1","place_id":"p1","open_time":"09:00","close_time":"09:30","stay_minutes":0},
            {"id":"2","place_id":"p2","open_time":"09:00","close_time":"23:50","stay_minutes":0},
            {"id":"3","place_id":"p3","open_time":"00:00","close_time":"23:50","stay_minutes":0}
        ],
        "start_time": "09:00"
    }))
    .unwrap();
    request.try_into().unwrap()
}

fn constrained_matrix() -> TravelTimeMatrix {
    TravelTimeMatrix::new(vec![
        vec![0, 10, 10, 10],
        vec![10, 0, 10, 10],
        vec![10, 30, 0, 10],
        vec![10, 10, 10, 0],
    ])
    .unwrap()
}

#[test]
fn penalty_evaluator_keeps_infeasible_routes_in_the_search_space() {
    let problem = constrained_problem();
    let matrix = constrained_matrix();
    let evaluation = evaluate_solution_with_penalties(
        &DefaultObjectivePolicy,
        SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        },
        &SolverSolution {
            visit_order: vec![0, 2, 1, 3],
        },
        InfeasiblePenaltyWeights::default(),
    )
    .unwrap();

    assert!(!evaluation.feasible);
    assert!(evaluation.late_minutes > 0);
    assert!(evaluation.penalty > 0.0);
    assert!(evaluation.energy.is_finite());
}

#[derive(Clone)]
struct FixedInitial(Vec<usize>);

impl InitialRouteStrategy for FixedInitial {
    fn initial_solution(
        &self,
        _input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        Ok(SolverSolution {
            visit_order: self.0.clone(),
        })
    }
}

#[derive(Default)]
struct AlwaysSwap;

impl NeighborhoodStrategy for AlwaysSwap {
    fn neighbor(
        &self,
        route: &[usize],
        _rng: &mut dyn RngCore,
    ) -> Option<(Vec<usize>, NeighborhoodMove)> {
        let movement = NeighborhoodMove::Swap { left: 1, right: 2 };
        let mut neighbor = route.to_vec();
        apply_neighborhood_move(&mut neighbor, movement).then_some((neighbor, movement))
    }
}

#[test]
fn infeasible_states_can_be_accepted_but_best_feasible_is_returned() {
    let problem = constrained_problem();
    let matrix = constrained_matrix();
    let config = SimulatedAnnealingConfig {
        initial_temperature: 1.0e12,
        cooling_rate: 0.99,
        minimum_temperature: 1.0,
        iteration_limit: Some(20),
        time_limit_ms: None,
        seed: Some(7),
        penalties: InfeasiblePenaltyWeights {
            late_minute_weight: 1.0,
            ..InfeasiblePenaltyWeights::default()
        },
    };
    let solver = SimulatedAnnealingSolver::new(
        config,
        FixedInitial(vec![0, 1, 2, 3]),
        AlwaysSwap,
        DefaultObjectivePolicy,
    )
    .unwrap();
    let result = solver
        .solve_detailed(SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        })
        .unwrap();

    assert!(result.stats.infeasible_candidates > 0);
    assert!(result.stats.accepted_infeasible_moves > 0);
    assert_eq!(result.solution.visit_order, vec![0, 1, 2, 3]);
    assert!(evaluate_solution(
        &DefaultObjectivePolicy,
        SolverInput {
            matrix: &matrix,
            problem: &problem,
            cancellation: &CancellationToken::new(),
        },
        &result.solution,
    )
    .is_ok());
}
