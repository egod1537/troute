use crate::solver::SolverError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MstEdge {
    pub from: usize,
    pub to: usize,
    pub distance: u64,
}

/// Prim's algorithm with stable index tie-breaking.
pub(crate) fn minimum_spanning_tree<F>(
    node_count: usize,
    mut distance: F,
) -> Result<Vec<MstEdge>, SolverError>
where
    F: FnMut(usize, usize) -> Result<u64, SolverError>,
{
    if node_count == 0 {
        return Err(SolverError::InvalidConfiguration(
            "an MST requires at least one node".to_owned(),
        ));
    }
    let mut included = vec![false; node_count];
    let mut best_distance = vec![u64::MAX; node_count];
    let mut parent = vec![None; node_count];
    best_distance[0] = 0;
    let mut edges = Vec::with_capacity(node_count.saturating_sub(1));

    for _ in 0..node_count {
        let next = (0..node_count)
            .filter(|&node| !included[node])
            .min_by_key(|&node| (best_distance[node], node))
            .ok_or_else(|| SolverError::Failed("MST construction failed".to_owned()))?;
        if best_distance[next] == u64::MAX {
            return Err(SolverError::Failed("MST graph is not connected".to_owned()));
        }
        included[next] = true;
        if let Some(from) = parent[next] {
            edges.push(MstEdge {
                from,
                to: next,
                distance: best_distance[next],
            });
        }

        for candidate in 0..node_count {
            if included[candidate] || candidate == next {
                continue;
            }
            let candidate_distance = distance(next, candidate)?;
            let better_parent = match parent[candidate] {
                Some(old_parent) => next < old_parent,
                None => true,
            };
            let is_better = candidate_distance < best_distance[candidate]
                || (candidate_distance == best_distance[candidate] && better_parent);
            if is_better {
                best_distance[candidate] = candidate_distance;
                parent[candidate] = Some(next);
            }
        }
    }
    Ok(edges)
}
