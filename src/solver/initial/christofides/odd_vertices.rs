use crate::solver::{MstEdge, SolverError};

pub fn odd_degree_vertices(
    node_count: usize,
    edges: &[MstEdge],
) -> Result<Vec<usize>, SolverError> {
    let mut degrees = vec![0_usize; node_count];
    for edge in edges {
        if edge.from >= node_count || edge.to >= node_count || edge.from == edge.to {
            return Err(SolverError::InvalidConfiguration(
                "graph edge contains an invalid endpoint".to_owned(),
            ));
        }
        degrees[edge.from] += 1;
        degrees[edge.to] += 1;
    }
    Ok(degrees
        .into_iter()
        .enumerate()
        .filter_map(|(vertex, degree)| (degree % 2 == 1).then_some(vertex))
        .collect())
}
