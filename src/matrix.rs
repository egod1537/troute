use thiserror::Error;

mod service;

pub use service::{MatrixService, MatrixSource};

/// A row-major directed matrix where `None` is a known unreachable arc.
///
/// Zero remains a valid travel time, including on non-diagonal arcs, and is
/// never used as an unreachable sentinel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TravelTimeMatrix {
    rows: Vec<Vec<Option<u32>>>,
}

impl TravelTimeMatrix {
    pub fn new(rows: Vec<Vec<u32>>) -> Result<Self, MatrixError> {
        Self::new_sparse(
            rows.into_iter()
                .map(|row| row.into_iter().map(Some).collect())
                .collect(),
        )
    }

    pub fn new_sparse(rows: Vec<Vec<Option<u32>>>) -> Result<Self, MatrixError> {
        if rows.is_empty() {
            return Err(MatrixError::Empty);
        }

        let size = rows.len();
        if let Some((row, actual)) = rows
            .iter()
            .enumerate()
            .find_map(|(row, values)| (values.len() != size).then_some((row, values.len())))
        {
            return Err(MatrixError::NotSquare {
                row,
                expected: size,
                actual,
            });
        }

        for (index, row) in rows.iter().enumerate() {
            if row[index] != Some(0) {
                return Err(MatrixError::InvalidDiagonal {
                    index,
                    actual: row[index],
                });
            }
        }

        Ok(Self { rows })
    }

    pub fn size(&self) -> usize {
        self.rows.len()
    }

    pub fn travel_minutes(&self, from: usize, to: usize) -> Option<u32> {
        self.rows.get(from)?.get(to).copied().flatten()
    }

    pub fn into_rows(self) -> Vec<Vec<Option<u32>>> {
        self.rows
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MatrixError {
    #[error("travel time matrix must not be empty")]
    Empty,
    #[error("travel time matrix row {row} has length {actual}; expected {expected}")]
    NotSquare {
        row: usize,
        expected: usize,
        actual: usize,
    },
    #[error("travel time matrix diagonal entry [{index}][{index}] must be 0; actual {actual:?}")]
    InvalidDiagonal { index: usize, actual: Option<u32> },
}

#[cfg(test)]
mod tests;
