use rand::{Rng, RngCore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeighborhoodMove {
    Swap { left: usize, right: usize },
    Relocate { from: usize, to: usize },
    TwoOpt { start: usize, end: usize },
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
        _ => false,
    }
}

pub trait NeighborhoodStrategy: Send + Sync {
    fn neighbor(
        &self,
        route: &[usize],
        rng: &mut dyn RngCore,
    ) -> Option<(Vec<usize>, NeighborhoodMove)>;
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
}
