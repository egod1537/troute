use crate::{
    domain::{Location, OptimizationProblem, StartPolicy, TimeOfDay, TimeWindow},
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
    let all_day = TimeWindow::new(
        TimeOfDay::from_minutes(0).map_err(|error| SolverError::Failed(error.to_string()))?,
        TimeOfDay::from_minutes(1439).map_err(|error| SolverError::Failed(error.to_string()))?,
    )
    .map_err(|error| SolverError::Failed(error.to_string()))?;
    // `start_slot` is the departure time after `previous` has already been
    // opened, waited at, and serviced by the outer traversal. Represent that
    // completed state with a zero-service anchor so the subproblem cannot
    // charge the location's stay a second time.
    let completed_anchor = Location::new(
        "__troute_cluster_entry__".to_owned(),
        input.problem.locations()[previous]
            .routing_reference()
            .clone(),
        all_day,
        0,
    );
    let dummy = Location::new(
        "__troute_cluster_exit__".to_owned(),
        input.problem.locations()[previous]
            .routing_reference()
            .clone(),
        all_day,
        0,
    );
    let mut locations = Vec::with_capacity(cluster.members.len() + 2);
    locations.push(completed_anchor);
    locations.extend(cluster.members.iter().map(|&member| {
        input.problem.locations()[member]
            .clone()
            .with_order_lock(false)
    }));
    locations.push(dummy);
    let subproblem = OptimizationProblem::with_start_policy(
        locations,
        TimeOfDay::from_minutes(start_slot * TIME_SLOT_MINUTES as u16)
            .map_err(|error| SolverError::Failed(error.to_string()))?,
        // The outer traversal owns the clock. Letting this subproblem select a
        // later start would optimize an ordering for a time that is never
        // propagated back to the outer route.
        StartPolicy::Fixed,
    );

    let dummy_index = cluster.members.len() + 1;
    let mut mapping = Vec::with_capacity(dummy_index);
    mapping.push(previous);
    mapping.extend(cluster.members.iter().copied());
    let mut rows = vec![vec![None; dummy_index + 1]; dummy_index + 1];
    for (index, row) in rows.iter_mut().enumerate() {
        row[index] = Some(0);
    }
    for from in 0..dummy_index {
        for to in 0..dummy_index {
            if from != to {
                rows[from][to] = input.matrix.travel_minutes(mapping[from], mapping[to]);
            }
        }
        rows[from][dummy_index] = next_candidates
            .iter()
            .filter_map(|&next| input.matrix.travel_minutes(mapping[from], next))
            .min();
    }
    let submatrix = TravelTimeMatrix::new_sparse(rows)
        .map_err(|error| SolverError::Failed(error.to_string()))?;
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

#[cfg(test)]
mod tests;
