use crate::domain::VisitPolicy;

use super::transition::{initial_service, transition_time};
use crate::solver::{SolverError, SolverInput, SolverSolution};

/// One earliest departure per (visited set, last location) is sufficient when
/// travel times are static and waiting is allowed. No objective frontier is kept.
pub(in crate::solver) fn find_feasible_at_start(
    input: &SolverInput<'_>,
    start: u16,
    require_all: bool,
) -> Result<Option<SolverSolution>, SolverError> {
    find_feasible_at_start_until(input, start, require_all, None)
}

pub(in crate::solver) fn find_feasible_at_start_until(
    input: &SolverInput<'_>,
    start: u16,
    require_all: bool,
    deadline: Option<std::time::Instant>,
) -> Result<Option<SolverSolution>, SolverError> {
    super::super::feasibility::check_active(input, deadline)?;
    let Some(initial) = initial_service(input, start) else {
        return Ok(None);
    };
    let n = input.problem.locations().len();
    if n == 1 {
        return Ok(Some(SolverSolution {
            visit_order: vec![0],
        }));
    }
    let full_mask = (1_usize << n) - 1;
    let required_mask = if require_all || input.problem.visit_policy() == VisitPolicy::AllRequired {
        full_mask
    } else {
        input
            .problem
            .locations()
            .iter()
            .enumerate()
            .fold(0, |mask, (i, location)| {
                mask | if location.required() { 1 << i } else { 0 }
            })
    };
    let end = input.problem.end_location_index();
    let mut earliest = vec![u16::MAX; (full_mask + 1) * n];
    let mut parents = vec![u8::MAX; earliest.len()];
    earliest[n] = initial.departure_slot;

    for mask in 1..=full_mask {
        super::super::feasibility::check_active(input, deadline)?;
        for last in 0..end {
            let departure = earliest[mask * n + last];
            if departure == u16::MAX {
                continue;
            }
            for next in 1..n {
                let bit = 1 << next;
                let next_mask = mask | bit;
                if mask & bit != 0
                    || !input
                        .problem
                        .allows_visit_at(next, mask.count_ones() as usize)
                    || (next == end && next_mask & required_mask != required_mask)
                {
                    continue;
                }
                let Some(finish) = transition_time(input, last, next, departure) else {
                    continue;
                };
                let cell = next_mask * n + next;
                if finish >= earliest[cell] {
                    continue;
                }
                earliest[cell] = finish;
                parents[cell] = last as u8;
                if next == end {
                    return Ok(Some(reconstruct(&parents, n, next_mask, end)));
                }
            }
        }
    }
    Ok(None)
}

fn reconstruct(parents: &[u8], n: usize, mut mask: usize, mut last: usize) -> SolverSolution {
    let mut visit_order = vec![last];
    while last != 0 {
        let previous = usize::from(parents[mask * n + last]);
        mask ^= 1 << last;
        last = previous;
        visit_order.push(last);
    }
    visit_order.reverse();
    SolverSolution { visit_order }
}
