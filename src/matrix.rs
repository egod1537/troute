use thiserror::Error;

/// A row-major matrix where each value is travel time in minutes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TravelTimeMatrix {
    rows: Vec<Vec<u32>>,
}

impl TravelTimeMatrix {
    pub fn new(rows: Vec<Vec<u32>>) -> Result<Self, MatrixError> {
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

        Ok(Self { rows })
    }

    pub fn size(&self) -> usize {
        self.rows.len()
    }

    pub fn travel_minutes(&self, from: usize, to: usize) -> Option<u32> {
        self.rows.get(from)?.get(to).copied()
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
}
