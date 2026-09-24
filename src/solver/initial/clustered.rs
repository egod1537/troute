use rand::RngCore;

use super::{
    clone_input, validate_generator_input, GreedyInitialRoute, InitialRouteGenerator,
    MstDoubleTreeInitialRoute, RandomInitialRoute,
};
use crate::solver::{ClusteredSolver, RouteSolver, SolverError, SolverInput, SolverSolution};

#[derive(Debug, Clone)]
pub struct ClusteredInitialRoute<S = ClusteredSolver> {
    solver: S,
}

impl Default for ClusteredInitialRoute {
    fn default() -> Self {
        Self {
            solver: ClusteredSolver::default(),
        }
    }
}

impl<S> ClusteredInitialRoute<S> {
    pub fn new(solver: S) -> Self {
        Self { solver }
    }
}

impl<S: RouteSolver + Send + Sync> InitialRouteGenerator for ClusteredInitialRoute<S> {
    fn generate(
        &self,
        input: SolverInput<'_>,
        _rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        self.solver.solve(input)
    }
}

/// Default SA initializer: feasible clustered result first, then deterministic
/// MST double-tree, directed greedy, and finally a random permutation.
#[derive(Debug, Clone, Default)]
pub struct ClusteredMstGreedyInitialRoute {
    clustered: ClusteredInitialRoute,
    mst: MstDoubleTreeInitialRoute,
    greedy: GreedyInitialRoute,
    random: RandomInitialRoute,
}

impl InitialRouteGenerator for ClusteredMstGreedyInitialRoute {
    fn generate(
        &self,
        input: SolverInput<'_>,
        rng: &mut dyn RngCore,
    ) -> Result<SolverSolution, SolverError> {
        validate_generator_input(&input)?;
        for generator in [
            &self.clustered as &dyn InitialRouteGenerator,
            &self.mst,
            &self.greedy,
        ] {
            match generator.generate(clone_input(&input), rng) {
                Ok(solution) => return Ok(solution),
                Err(SolverError::Cancelled) => return Err(SolverError::Cancelled),
                Err(_) => {}
            }
        }
        self.random.generate(input, rng)
    }
}
