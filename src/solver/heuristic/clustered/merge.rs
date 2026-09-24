use crate::{
    domain::{Location, OptimizationProblem, TimeOfDay, TimeWindow},
    matrix::TravelTimeMatrix,
    solver::{
        Cluster, ExactBitDpSolver, FrontierPolicy, ObjectivePolicy, SolverError, SolverInput,
        TIME_SLOT_MINUTES,
    },
};

pub(super) fn solve_cluster<O: ObjectivePolicy, F: FrontierPolicy>(
    exact: &ExactBitDpSolver<O, F>,
    input: &SolverInput<'_>,
    previous: usize,
    start_slot: u16,
    cluster: &Cluster,
    next_candidates: &[usize],
) -> Result<SolvedCluster, SolverError> {
    let dummy = Location::new(
        "__troute_cluster_exit__".to_owned(),
        input.problem.locations()[previous]
            .routing_reference()
            .clone(),
        TimeWindow::new(
            TimeOfDay::from_minutes(0).map_err(|error| SolverError::Failed(error.to_string()))?,
            TimeOfDay::from_minutes(1439)
                .map_err(|error| SolverError::Failed(error.to_string()))?,
        )
        .map_err(|error| SolverError::Failed(error.to_string()))?,
        0,
    );
    let mut locations = Vec::with_capacity(cluster.members.len() + 2);
    locations.push(input.problem.locations()[previous].clone());
    locations.extend(
        cluster
            .members
            .iter()
            .map(|&member| input.problem.locations()[member].clone()),
    );
    locations.push(dummy);
    let subproblem = OptimizationProblem::new(
        locations,
        TimeOfDay::from_minutes(start_slot * TIME_SLOT_MINUTES as u16)
            .map_err(|error| SolverError::Failed(error.to_string()))?,
    );

    let dummy_index = cluster.members.len() + 1;
    let mut mapping = Vec::with_capacity(dummy_index);
    mapping.push(previous);
    mapping.extend(cluster.members.iter().copied());
    let mut rows = vec![vec![0_u32; dummy_index + 1]; dummy_index + 1];
    for from in 0..dummy_index {
        for to in 0..dummy_index {
            if from != to {
                rows[from][to] = input
                    .matrix
                    .travel_minutes(mapping[from], mapping[to])
                    .ok_or(SolverError::MatrixSizeMismatch {
                        matrix: input.matrix.size(),
                        locations: input.problem.locations().len(),
                    })?;
            }
        }
        rows[from][dummy_index] = next_candidates
            .iter()
            .filter_map(|&next| input.matrix.travel_minutes(mapping[from], next))
            .min()
            .ok_or(SolverError::NoFeasibleRoute)?;
    }
    let submatrix =
        TravelTimeMatrix::new(rows).map_err(|error| SolverError::Failed(error.to_string()))?;
    let detailed = exact.solve_detailed(SolverInput {
        matrix: &submatrix,
        problem: &subproblem,
        cancellation: input.cancellation,
    })?;
    let order = detailed.solution.visit_order[1..detailed.solution.visit_order.len() - 1]
        .iter()
        .map(|&sub_index| cluster.members[sub_index - 1])
        .collect();
    Ok(SolvedCluster {
        order,
        generated_states: detailed.stats.generated_states,
        frontier_states: detailed.stats.frontier_states,
    })
}

pub(super) struct SolvedCluster {
    pub(super) order: Vec<usize>,
    pub(super) generated_states: usize,
    pub(super) frontier_states: usize,
}
