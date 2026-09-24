use crate::solver::SolverError;

use super::MstEdge;

/// Returns the deterministic Euler walk of the multigraph obtained by
/// doubling every tree edge.
pub fn doubled_tree_euler_tour(
    node_count: usize,
    edges: &[MstEdge],
    root: usize,
) -> Result<Vec<usize>, SolverError> {
    if node_count == 0 || root >= node_count || edges.len() != node_count.saturating_sub(1) {
        return Err(SolverError::InvalidConfiguration(
            "Euler tour input must be a rooted spanning tree".to_owned(),
        ));
    }
    let mut adjacency = vec![Vec::<(usize, u64)>::new(); node_count];
    for edge in edges {
        if edge.from >= node_count || edge.to >= node_count || edge.from == edge.to {
            return Err(SolverError::InvalidConfiguration(
                "MST edge contains an invalid endpoint".to_owned(),
            ));
        }
        adjacency[edge.from].push((edge.to, edge.distance));
        adjacency[edge.to].push((edge.from, edge.distance));
    }
    for neighbors in &mut adjacency {
        neighbors.sort_by_key(|&(node, distance)| (distance, node));
    }

    let mut seen = vec![false; node_count];
    seen[root] = true;
    let mut tour = vec![root];
    let mut stack = vec![(root, 0_usize)];
    while let Some((node, next_neighbor)) = stack.last_mut() {
        if *next_neighbor < adjacency[*node].len() {
            let child = adjacency[*node][*next_neighbor].0;
            *next_neighbor += 1;
            if !seen[child] {
                seen[child] = true;
                tour.push(child);
                stack.push((child, 0));
            }
        } else {
            stack.pop();
            if let Some((parent, _)) = stack.last() {
                tour.push(*parent);
            }
        }
    }
    if seen.iter().any(|visited| !visited) {
        return Err(SolverError::InvalidConfiguration(
            "MST edges do not connect every node".to_owned(),
        ));
    }
    Ok(tour)
}

pub fn shortcut_euler_tour(
    node_count: usize,
    euler_tour: &[usize],
) -> Result<Vec<usize>, SolverError> {
    let mut seen = vec![false; node_count];
    let mut route = Vec::with_capacity(node_count);
    for &node in euler_tour {
        if node >= node_count {
            return Err(SolverError::InvalidVisitOrder);
        }
        if !seen[node] {
            seen[node] = true;
            route.push(node);
        }
    }
    if route.len() != node_count {
        return Err(SolverError::InvalidVisitOrder);
    }
    Ok(route)
}
