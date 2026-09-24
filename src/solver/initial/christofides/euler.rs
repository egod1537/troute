use crate::solver::{MstEdge, SolverError};

/// Hierholzer traversal for a connected undirected Eulerian multigraph.
/// Parallel edges are represented by repeated `MstEdge` entries.
pub fn eulerian_multigraph_tour(
    node_count: usize,
    edges: &[MstEdge],
    root: usize,
) -> Result<Vec<usize>, SolverError> {
    if node_count == 0 || root >= node_count {
        return Err(SolverError::InvalidConfiguration(
            "Euler tour root must be inside a non-empty graph".to_owned(),
        ));
    }
    let mut adjacency = vec![Vec::<(usize, u64, usize)>::new(); node_count];
    let mut degrees = vec![0_usize; node_count];
    for (edge_id, edge) in edges.iter().enumerate() {
        if edge.from >= node_count || edge.to >= node_count || edge.from == edge.to {
            return Err(SolverError::InvalidConfiguration(
                "Euler graph edge contains an invalid endpoint".to_owned(),
            ));
        }
        adjacency[edge.from].push((edge.to, edge.distance, edge_id));
        adjacency[edge.to].push((edge.from, edge.distance, edge_id));
        degrees[edge.from] += 1;
        degrees[edge.to] += 1;
    }
    if degrees.iter().any(|degree| degree % 2 != 0) {
        return Err(SolverError::InvalidConfiguration(
            "Euler graph must have even degree at every vertex".to_owned(),
        ));
    }
    for neighbors in &mut adjacency {
        neighbors.sort_by_key(|&(vertex, distance, edge_id)| (distance, vertex, edge_id));
    }

    let mut used_edges = vec![false; edges.len()];
    let mut cursors = vec![0_usize; node_count];
    let mut stack = vec![root];
    let mut reverse_tour = Vec::with_capacity(edges.len() + 1);
    while let Some(&vertex) = stack.last() {
        while cursors[vertex] < adjacency[vertex].len()
            && used_edges[adjacency[vertex][cursors[vertex]].2]
        {
            cursors[vertex] += 1;
        }
        if let Some(&(next, _, edge_id)) = adjacency[vertex].get(cursors[vertex]) {
            cursors[vertex] += 1;
            used_edges[edge_id] = true;
            stack.push(next);
        } else {
            reverse_tour.push(vertex);
            stack.pop();
        }
    }
    let mut reached_vertices = vec![false; node_count];
    for &vertex in &reverse_tour {
        reached_vertices[vertex] = true;
    }
    if used_edges.iter().any(|used| !used)
        || reached_vertices.iter().any(|reached| !reached)
        || reverse_tour.len() != edges.len() + 1
    {
        return Err(SolverError::InvalidConfiguration(
            "Euler graph must be connected".to_owned(),
        ));
    }
    reverse_tour.reverse();
    Ok(reverse_tour)
}
