use crate::solver::{InfeasiblePenaltyWeights, SolverError};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimulatedAnnealingConfig {
    pub initial_temperature: f64,
    pub cooling_rate: f64,
    pub minimum_temperature: f64,
    pub iteration_limit: Option<u64>,
    pub time_limit_ms: Option<u64>,
    pub seed: Option<u64>,
    pub penalties: InfeasiblePenaltyWeights,
}

impl Default for SimulatedAnnealingConfig {
    fn default() -> Self {
        Self {
            initial_temperature: 1_000.0,
            cooling_rate: 0.995,
            minimum_temperature: 0.01,
            iteration_limit: Some(10_000),
            time_limit_ms: None,
            seed: None,
            penalties: InfeasiblePenaltyWeights::default(),
        }
    }
}

impl SimulatedAnnealingConfig {
    pub fn validate(self) -> Result<Self, SolverError> {
        if !self.initial_temperature.is_finite() || self.initial_temperature <= 0.0 {
            return Err(SolverError::InvalidConfiguration(
                "initial_temperature must be finite and positive".to_owned(),
            ));
        }
        if !self.cooling_rate.is_finite() || !(0.0..1.0).contains(&self.cooling_rate) {
            return Err(SolverError::InvalidConfiguration(
                "cooling_rate must be finite and between 0 and 1".to_owned(),
            ));
        }
        if !self.minimum_temperature.is_finite()
            || self.minimum_temperature <= 0.0
            || self.minimum_temperature >= self.initial_temperature
        {
            return Err(SolverError::InvalidConfiguration(
                "minimum_temperature must be positive and below initial_temperature".to_owned(),
            ));
        }
        if self.iteration_limit == Some(0) {
            return Err(SolverError::InvalidConfiguration(
                "iteration_limit must be positive when set".to_owned(),
            ));
        }
        if self.time_limit_ms == Some(0) {
            return Err(SolverError::InvalidConfiguration(
                "time_limit_ms must be positive when set".to_owned(),
            ));
        }
        self.penalties.validate()?;
        Ok(self)
    }
}
