use rand::{Rng, RngCore};

use crate::domain::{OptimizationProblem, VisitPolicy};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeighborhoodMove {
    Swap { left: usize, right: usize },
    Relocate { from: usize, to: usize },
    TwoOpt { start: usize, end: usize },
    InsertOptional { location: usize, position: usize },
    RemoveOptional { position: usize },
    ReplaceOptional { position: usize, location: usize },
}

pub fn apply_neighborhood_move(route: &mut Vec<usize>, movement: NeighborhoodMove) -> bool {
    let is_interior = |index: usize| index > 0 && index + 1 < route.len();
    match movement {
        NeighborhoodMove::Swap { left, right }
            if left != right && is_interior(left) && is_interior(right) =>
        {
            route.swap(left, right);
            true
        }
        NeighborhoodMove::Relocate { from, to }
            if from != to && is_interior(from) && is_interior(to) =>
        {
            let location = route.remove(from);
            route.insert(to, location);
            true
        }
        NeighborhoodMove::TwoOpt { start, end }
            if start < end && is_interior(start) && is_interior(end) =>
        {
            route[start..=end].reverse();
            true
        }
        NeighborhoodMove::InsertOptional { location, position }
            if position > 0 && position < route.len() && !route.contains(&location) =>
        {
            route.insert(position, location);
            true
        }
        NeighborhoodMove::RemoveOptional { position } if is_interior(position) => {
            route.remove(position);
            true
        }
        NeighborhoodMove::ReplaceOptional { position, location }
            if is_interior(position) && !route.contains(&location) =>
        {
            route[position] = location;
            true
        }
        _ => false,
    }
}

pub trait NeighborhoodStrategy: Send + Sync {
    fn neighbor(
        &self,
        route: &[usize],
        rng: &mut dyn RngCore,
    ) -> Option<(Vec<usize>, NeighborhoodMove)>;

    fn neighbor_for_problem(
        &self,
        route: &[usize],
        _problem: &OptimizationProblem,
        rng: &mut dyn RngCore,
    ) -> Option<(Vec<usize>, NeighborhoodMove)> {
        self.neighbor(route, rng)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MixedNeighborhoodStrategy;

impl NeighborhoodStrategy for MixedNeighborhoodStrategy {
    fn neighbor(
        &self,
        route: &[usize],
        rng: &mut dyn RngCore,
    ) -> Option<(Vec<usize>, NeighborhoodMove)> {
        if route.len() < 4 {
            return None;
        }
        let left = rng.gen_range(1..route.len() - 1);
        let mut right = rng.gen_range(1..route.len() - 1);
        while left == right {
            right = rng.gen_range(1..route.len() - 1);
        }
        let movement = match rng.gen_range(0..3) {
            0 => NeighborhoodMove::Swap { left, right },
            1 => NeighborhoodMove::Relocate {
                from: left,
                to: right,
            },
            _ => NeighborhoodMove::TwoOpt {
                start: left.min(right),
                end: left.max(right),
            },
        };
        let mut neighbor = route.to_vec();
        apply_neighborhood_move(&mut neighbor, movement).then_some((neighbor, movement))
    }

    fn neighbor_for_problem(
        &self,
        route: &[usize],
        problem: &OptimizationProblem,
        rng: &mut dyn RngCore,
    ) -> Option<(Vec<usize>, NeighborhoodMove)> {
        if problem.visit_policy() == VisitPolicy::AllRequired {
            return self.neighbor(route, rng);
        }
        let unvisited: Vec<_> = (1..problem.end_location_index())
            .filter(|&index| !problem.locations()[index].required() && !route.contains(&index))
            .collect();
        let removable: Vec<_> = (1..route.len().saturating_sub(1))
            .filter(|&position| !problem.locations()[route[position]].required())
            .collect();
        let mut kinds = Vec::new();
        if route.len() >= 4 {
            kinds.push(0_u8);
        }
        if !unvisited.is_empty() {
            kinds.push(1);
        }
        if !removable.is_empty() {
            kinds.push(2);
        }
        if !unvisited.is_empty() && !removable.is_empty() {
            kinds.push(3);
        }
        if kinds.is_empty() {
            return None;
        }
        let kind = *kinds.get(rng.gen_range(0..kinds.len()))?;
        let movement = match kind {
            0 => return self.neighbor(route, rng),
            1 => NeighborhoodMove::InsertOptional {
                location: unvisited[rng.gen_range(0..unvisited.len())],
                position: rng.gen_range(1..route.len()),
            },
            2 => NeighborhoodMove::RemoveOptional {
                position: removable[rng.gen_range(0..removable.len())],
            },
            _ => NeighborhoodMove::ReplaceOptional {
                position: removable[rng.gen_range(0..removable.len())],
                location: unvisited[rng.gen_range(0..unvisited.len())],
            },
        };
        let mut neighbor = route.to_vec();
        apply_neighborhood_move(&mut neighbor, movement).then_some((neighbor, movement))
    }
}
