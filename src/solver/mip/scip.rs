use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use russcip::{prelude::*, ProblemCreated, Solution, Status, Variable};

use crate::{
    domain::StartPolicy,
    solver::{
        OrchestratedStrategy, ScipMipConfig, SolverCandidateMetadata, SolverError, SolverInput,
        SolverOutcomeKind, SolverProofStatus, SolverSolution, StrategyExecution,
    },
};

const MINUTES_PER_DAY: isize = 24 * 60;
const MAX_SCIP_LOCATIONS: usize = 100;
const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(2);

#[derive(Debug, Clone)]
pub struct ScipMipStrategy {
    config: ScipMipConfig,
}

impl ScipMipStrategy {
    pub fn new(config: ScipMipConfig) -> Result<Self, SolverError> {
        if config.time_limit.is_zero() || config.threads == 0 || config.memory_mb == 0 {
            return Err(SolverError::InvalidConfiguration(
                "SCIP time, thread, and memory limits must be positive".to_owned(),
            ));
        }
        Ok(Self { config })
    }
}

impl OrchestratedStrategy for ScipMipStrategy {
    fn name(&self) -> &str {
        "mip_scip"
    }

    fn is_applicable(&self, location_count: usize) -> bool {
        (2..=MAX_SCIP_LOCATIONS).contains(&location_count)
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        solve_lexicographic(input, &self.config)
    }
}

#[derive(Debug, Clone, Copy)]
enum ObjectivePhase {
    Coverage,
    Priority,
    LatestStart,
    Finish,
    Travel,
    Wait,
}

impl ObjectivePhase {
    fn maximize(self) -> bool {
        matches!(self, Self::Coverage | Self::Priority | Self::LatestStart)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct FixedObjectives {
    coverage: Option<i64>,
    priority: Option<i64>,
    start: Option<i64>,
    finish: Option<i64>,
    travel: Option<i64>,
}

struct ModelVariables {
    y: Vec<Variable>,
    service_start: Vec<Variable>,
    selected_start: Variable,
    arcs: Vec<Vec<Option<Variable>>>,
}

struct PhaseResult {
    solution: SolverSolution,
    value: i64,
    status: Status,
    best_bound: f64,
    phase: ObjectivePhase,
}

fn solve_lexicographic(
    input: SolverInput<'_>,
    config: &ScipMipConfig,
) -> Result<StrategyExecution, SolverError> {
    let started = Instant::now();
    let phases = match input.problem.start_policy() {
        StartPolicy::Latest => vec![
            ObjectivePhase::Coverage,
            ObjectivePhase::Priority,
            ObjectivePhase::LatestStart,
            ObjectivePhase::Finish,
            ObjectivePhase::Travel,
            ObjectivePhase::Wait,
        ],
        StartPolicy::Fixed | StartPolicy::Earliest => vec![
            ObjectivePhase::Coverage,
            ObjectivePhase::Priority,
            ObjectivePhase::Finish,
            ObjectivePhase::Travel,
            ObjectivePhase::Wait,
        ],
    };
    let mut fixed = FixedObjectives::default();
    let mut final_result = None;

    for phase in phases {
        if input.cancellation.is_cancelled() {
            return Err(SolverError::Cancelled);
        }
        let remaining = config.time_limit.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return final_result
                .map(|result| execution(result, config, false, true))
                .unwrap_or(Err(SolverError::TimedOut));
        }
        let result = solve_phase(input, config, remaining, phase, fixed)?;
        let proven = result.status == Status::Optimal;
        match phase {
            ObjectivePhase::Coverage => fixed.coverage = Some(result.value),
            ObjectivePhase::Priority => fixed.priority = Some(result.value),
            ObjectivePhase::LatestStart => fixed.start = Some(result.value),
            ObjectivePhase::Finish => fixed.finish = Some(result.value),
            ObjectivePhase::Travel => fixed.travel = Some(result.value),
            ObjectivePhase::Wait => {}
        }
        final_result = Some(result);
        if !proven {
            return execution(
                final_result.expect("phase result was stored"),
                config,
                false,
                true,
            );
        }
    }

    execution(
        final_result.expect("at least one SCIP objective phase is configured"),
        config,
        true,
        false,
    )
}

fn execution(
    result: PhaseResult,
    config: &ScipMipConfig,
    proven: bool,
    timed_out: bool,
) -> Result<StrategyExecution, SolverError> {
    let objective = result.value.unsigned_abs();
    let bound = finite_nonnegative_u64(result.best_bound.abs());
    let relative_gap_ppm = bound.map(|bound| {
        objective
            .abs_diff(bound)
            .saturating_mul(1_000_000)
            .checked_div(objective)
            .unwrap_or_else(|| u64::from(bound != 0) * 1_000_000)
    });
    Ok(StrategyExecution {
        solution: result.solution,
        metadata: SolverCandidateMetadata {
            outcome: Some(if timed_out {
                SolverOutcomeKind::Timeout
            } else {
                SolverOutcomeKind::Completed
            }),
            proof_status: Some(if proven {
                SolverProofStatus::Optimal
            } else {
                SolverProofStatus::Feasible
            }),
            objective_phase: Some(format!("{:?}", result.phase).to_ascii_uppercase()),
            best_bound: bound,
            relative_gap_ppm,
            thread_count: Some(config.threads),
            source_version: Some("SCIP 10.0.2 via russcip 0.11.0".to_owned()),
            native_status: Some(format!("{:?}", result.status).to_ascii_uppercase()),
            timed_out,
            ..SolverCandidateMetadata::default()
        },
    })
}

fn finite_nonnegative_u64(value: f64) -> Option<u64> {
    value
        .is_finite()
        .then(|| value.max(0.0).round().min(u64::MAX as f64) as u64)
}

fn solve_phase(
    input: SolverInput<'_>,
    config: &ScipMipConfig,
    remaining: Duration,
    phase: ObjectivePhase,
    fixed: FixedObjectives,
) -> Result<PhaseResult, SolverError> {
    let (model, variables) = build_model(input, config, remaining, phase, fixed)?;
    let raw_scip = model.scip_ptr() as usize;
    let done = AtomicBool::new(false);
    let solved = thread::scope(|scope| {
        let cancellation = input.cancellation;
        let done_ref = &done;
        let watcher = scope.spawn(move || {
            while !done_ref.load(Ordering::Acquire) {
                if cancellation.is_cancelled() {
                    // SCIP documents this as the public per-instance cooperative
                    // interruption API. The model itself stays on this thread.
                    unsafe {
                        russcip::ffi::SCIPinterruptSolve(raw_scip as *mut russcip::ffi::SCIP);
                    }
                    break;
                }
                thread::sleep(CANCELLATION_POLL_INTERVAL);
            }
        });
        let solved = model.solve();
        done.store(true, Ordering::Release);
        let _ = watcher.join();
        solved
    });

    if input.cancellation.is_cancelled() || solved.status() == Status::UserInterrupt {
        return Err(SolverError::Cancelled);
    }
    if solved.status() == Status::Infeasible {
        return if matches!(phase, ObjectivePhase::Coverage) {
            Err(SolverError::ProvenInfeasible)
        } else {
            Err(SolverError::Failed(format!(
                "SCIP became infeasible after fixing a prior objective phase: {phase:?}"
            )))
        };
    }
    let Some(solution) = solved.best_sol() else {
        return if solved.status() == Status::TimeLimit {
            Err(SolverError::TimedOut)
        } else {
            Err(SolverError::Failed(format!(
                "SCIP ended with {:?} and no incumbent",
                solved.status()
            )))
        };
    };
    let route = extract_route(input, &variables, &solution)?;
    let value = phase_value(input, phase, &variables, &solution);
    Ok(PhaseResult {
        solution: SolverSolution { visit_order: route },
        value,
        status: solved.status(),
        best_bound: solved.best_bound(),
        phase,
    })
}

fn build_model(
    input: SolverInput<'_>,
    config: &ScipMipConfig,
    remaining: Duration,
    phase: ObjectivePhase,
    fixed: FixedObjectives,
) -> Result<(Model<ProblemCreated>, ModelVariables), SolverError> {
    let n = input.problem.locations().len();
    let start = input.problem.start_location_index();
    let end = input.problem.end_location_index();
    let mut model = Model::default().hide_output();
    model = if phase.maximize() {
        model.maximize()
    } else {
        model.minimize()
    };
    model = model
        .set_real_param("limits/time", remaining.as_secs_f64().max(0.001))
        .map_err(|error| SolverError::Failed(format!("cannot set SCIP time limit: {error:?}")))?;
    model = model
        .set_real_param("limits/memory", config.memory_mb as f64)
        .map_err(|error| SolverError::Failed(format!("cannot set SCIP memory limit: {error:?}")))?;
    model = model
        .set_int_param(
            "parallel/maxnthreads",
            i32::try_from(config.threads).unwrap_or(i32::MAX),
        )
        .map_err(|error| SolverError::Failed(format!("cannot set SCIP thread limit: {error:?}")))?;
    model = model
        .set_int_param(
            "randomization/randomseedshift",
            i32::try_from(config.seed % i32::MAX as u64).unwrap_or(0),
        )
        .map_err(|error| SolverError::Failed(format!("cannot set SCIP seed: {error:?}")))?;

    let y = input
        .problem
        .locations()
        .iter()
        .enumerate()
        .map(|(index, location)| {
            let coefficient = match phase {
                ObjectivePhase::Coverage if !location.required() => 1.0,
                ObjectivePhase::Priority if !location.required() => f64::from(location.priority()),
                ObjectivePhase::Wait => -f64::from(location.stay_minutes()),
                _ => 0.0,
            };
            let name = format!("y_{index}");
            if location.required() {
                model.add(var().int(1..=1).obj(coefficient).name(&name))
            } else {
                model.add(var().bin().obj(coefficient).name(&name))
            }
        })
        .collect::<Vec<_>>();
    let service_start = (0..n)
        .map(|index| {
            let coefficient = match phase {
                ObjectivePhase::Finish if index == end => 1.0,
                ObjectivePhase::Wait if index == end => 1.0,
                _ => 0.0,
            };
            model.add(
                var()
                    .int(0..MINUTES_PER_DAY)
                    .obj(coefficient)
                    .name(&format!("s_{index}")),
            )
        })
        .collect::<Vec<_>>();
    let selected_start_coefficient = match phase {
        ObjectivePhase::LatestStart => 1.0,
        ObjectivePhase::Wait => -1.0,
        _ => 0.0,
    };
    let selected_start = model.add(
        var()
            .int(0..MINUTES_PER_DAY)
            .obj(selected_start_coefficient)
            .name("selected_start"),
    );
    let order = (0..n)
        .map(|index| {
            let bounds = if index == start {
                0..=0
            } else {
                0..=n as isize - 1
            };
            model.add(var().int(bounds).name(&format!("u_{index}")))
        })
        .collect::<Vec<_>>();
    let mut arcs = (0..n)
        .map(|_| (0..n).map(|_| None).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    for (from, row) in arcs.iter_mut().enumerate() {
        for (to, slot) in row.iter_mut().enumerate() {
            if from == to || to == start || from == end {
                continue;
            }
            let Some(travel) = input.matrix.travel_minutes(from, to) else {
                continue;
            };
            let coefficient = match phase {
                ObjectivePhase::Travel => f64::from(travel),
                ObjectivePhase::Wait => -f64::from(travel),
                _ => 0.0,
            };
            *slot = Some(model.add(var().bin().obj(coefficient).name(&format!("x_{from}_{to}"))));
        }
    }

    add_flow_constraints(&mut model, &arcs, &y, start, end);
    add_subtour_constraints(&mut model, &arcs, &order, n);
    add_time_constraints(
        &mut model,
        input,
        &arcs,
        &y,
        &service_start,
        &selected_start,
    );
    add_fixed_objective_constraints(
        &mut model,
        input,
        &arcs,
        &y,
        &service_start,
        &selected_start,
        fixed,
    );

    Ok((
        model,
        ModelVariables {
            y,
            service_start,
            selected_start,
            arcs,
        },
    ))
}

fn add_flow_constraints(
    model: &mut Model<ProblemCreated>,
    arcs: &[Vec<Option<Variable>>],
    y: &[Variable],
    start: usize,
    end: usize,
) {
    let n = arcs.len();
    for index in 0..n {
        let incoming = (0..n).filter_map(|from| arcs[from][index].as_ref());
        let outgoing = (0..n).filter_map(|to| arcs[index][to].as_ref());
        if index == start {
            model.add(cons().eq(1.0).expr(outgoing.map(|arc| (arc, 1.0))));
        } else if index == end {
            model.add(cons().eq(1.0).expr(incoming.map(|arc| (arc, 1.0))));
        } else {
            model.add(
                cons()
                    .eq(0.0)
                    .coef(&y[index], -1.0)
                    .expr(incoming.map(|arc| (arc, 1.0))),
            );
            model.add(
                cons()
                    .eq(0.0)
                    .coef(&y[index], -1.0)
                    .expr(outgoing.map(|arc| (arc, 1.0))),
            );
        }
    }
}

fn add_subtour_constraints(
    model: &mut Model<ProblemCreated>,
    arcs: &[Vec<Option<Variable>>],
    order: &[Variable],
    n: usize,
) {
    for from in 0..n {
        for to in 0..n {
            let Some(arc) = arcs[from][to].as_ref() else {
                continue;
            };
            model.add(
                cons()
                    .le(n as f64 - 1.0)
                    .coef(&order[from], 1.0)
                    .coef(&order[to], -1.0)
                    .coef(arc, n as f64),
            );
        }
    }
}

fn add_time_constraints(
    model: &mut Model<ProblemCreated>,
    input: SolverInput<'_>,
    arcs: &[Vec<Option<Variable>>],
    y: &[Variable],
    service_start: &[Variable],
    selected_start: &Variable,
) {
    for (index, location) in input.problem.locations().iter().enumerate() {
        let open = f64::from(location.time_window().open().minutes());
        let close = f64::from(location.time_window().close().minutes());
        let stay = f64::from(location.stay_minutes());
        let big_m = MINUTES_PER_DAY as f64 + stay;
        model.add(
            cons()
                .ge(open - big_m)
                .coef(&service_start[index], 1.0)
                .coef(&y[index], -big_m),
        );
        model.add(
            cons()
                .le(close + big_m - stay)
                .coef(&service_start[index], 1.0)
                .coef(&y[index], big_m),
        );
    }
    let requested = f64::from(input.problem.start_time().minutes());
    match input.problem.start_policy() {
        StartPolicy::Fixed | StartPolicy::Earliest => {
            model.add(cons().eq(requested).coef(selected_start, 1.0));
        }
        StartPolicy::Latest => {
            model.add(cons().ge(requested).coef(selected_start, 1.0));
        }
    }
    let start = input.problem.start_location_index();
    model.add(
        cons()
            .ge(0.0)
            .coef(&service_start[start], 1.0)
            .coef(selected_start, -1.0),
    );
    for from in 0..arcs.len() {
        let stay = f64::from(input.problem.locations()[from].stay_minutes());
        for to in 0..arcs.len() {
            let Some(arc) = arcs[from][to].as_ref() else {
                continue;
            };
            let travel = f64::from(
                input
                    .matrix
                    .travel_minutes(from, to)
                    .expect("an arc exists only for a reachable matrix entry"),
            );
            let big_m = MINUTES_PER_DAY as f64 + stay + travel;
            model.add(
                cons()
                    .ge(stay + travel - big_m)
                    .coef(&service_start[to], 1.0)
                    .coef(&service_start[from], -1.0)
                    .coef(arc, -big_m),
            );
        }
    }
}

fn add_fixed_objective_constraints(
    model: &mut Model<ProblemCreated>,
    input: SolverInput<'_>,
    arcs: &[Vec<Option<Variable>>],
    y: &[Variable],
    service_start: &[Variable],
    selected_start: &Variable,
    fixed: FixedObjectives,
) {
    if let Some(value) = fixed.coverage {
        model.add(
            cons().eq(value as f64).expr(
                input
                    .problem
                    .locations()
                    .iter()
                    .enumerate()
                    .filter(|(_, location)| !location.required())
                    .map(|(index, _)| (&y[index], 1.0)),
            ),
        );
    }
    if let Some(value) = fixed.priority {
        model.add(
            cons().eq(value as f64).expr(
                input
                    .problem
                    .locations()
                    .iter()
                    .enumerate()
                    .filter(|(_, location)| !location.required())
                    .map(|(index, location)| (&y[index], f64::from(location.priority()))),
            ),
        );
    }
    if let Some(value) = fixed.start {
        model.add(cons().eq(value as f64).coef(selected_start, 1.0));
    }
    if let Some(value) = fixed.finish {
        let end = input.problem.end_location_index();
        let service_start_value = value - i64::from(input.problem.locations()[end].stay_minutes());
        model.add(
            cons()
                .eq(service_start_value as f64)
                .coef(&service_start[end], 1.0),
        );
    }
    if let Some(value) = fixed.travel {
        model.add(
            cons()
                .eq(value as f64)
                .expr(arcs.iter().enumerate().flat_map(|(from, row)| {
                    row.iter().enumerate().filter_map(move |(to, arc)| {
                        arc.as_ref().map(|arc| {
                            (
                                arc,
                                f64::from(
                                    input
                                        .matrix
                                        .travel_minutes(from, to)
                                        .expect("a modeled arc is reachable"),
                                ),
                            )
                        })
                    })
                })),
        );
    }
}

fn phase_value(
    input: SolverInput<'_>,
    phase: ObjectivePhase,
    variables: &ModelVariables,
    solution: &Solution<'_>,
) -> i64 {
    let round = |value: f64| value.round() as i64;
    match phase {
        ObjectivePhase::Coverage => input
            .problem
            .locations()
            .iter()
            .enumerate()
            .filter(|(_, location)| !location.required())
            .map(|(index, _)| round(solution.val(&variables.y[index])))
            .sum(),
        ObjectivePhase::Priority => input
            .problem
            .locations()
            .iter()
            .enumerate()
            .filter(|(_, location)| !location.required())
            .map(|(index, location)| {
                round(solution.val(&variables.y[index])) * i64::from(location.priority())
            })
            .sum(),
        ObjectivePhase::LatestStart => round(solution.val(&variables.selected_start)),
        ObjectivePhase::Finish => {
            let end = input.problem.end_location_index();
            round(solution.val(&variables.service_start[end]))
                + i64::from(input.problem.locations()[end].stay_minutes())
        }
        ObjectivePhase::Travel => total_travel(input, variables, solution),
        ObjectivePhase::Wait => {
            let end = input.problem.end_location_index();
            let finish = round(solution.val(&variables.service_start[end]))
                + i64::from(input.problem.locations()[end].stay_minutes());
            let start = round(solution.val(&variables.selected_start));
            let stay: i64 = input
                .problem
                .locations()
                .iter()
                .enumerate()
                .map(|(index, location)| {
                    round(solution.val(&variables.y[index])) * i64::from(location.stay_minutes())
                })
                .sum();
            finish - start - stay - total_travel(input, variables, solution)
        }
    }
}

fn total_travel(
    input: SolverInput<'_>,
    variables: &ModelVariables,
    solution: &Solution<'_>,
) -> i64 {
    variables
        .arcs
        .iter()
        .enumerate()
        .flat_map(|(from, row)| {
            row.iter().enumerate().filter_map(move |(to, arc)| {
                arc.as_ref().map(|arc| {
                    (solution.val(arc).round() as i64)
                        * i64::from(
                            input
                                .matrix
                                .travel_minutes(from, to)
                                .expect("a modeled arc is reachable"),
                        )
                })
            })
        })
        .sum()
}

fn extract_route(
    input: SolverInput<'_>,
    variables: &ModelVariables,
    solution: &Solution<'_>,
) -> Result<Vec<usize>, SolverError> {
    let start = input.problem.start_location_index();
    let end = input.problem.end_location_index();
    let mut route = vec![start];
    let mut current = start;
    while current != end {
        let next = variables.arcs[current]
            .iter()
            .enumerate()
            .find_map(|(index, arc)| {
                arc.as_ref()
                    .filter(|arc| solution.val(arc) > 0.5)
                    .map(|_| index)
            })
            .ok_or_else(|| {
                SolverError::Failed("SCIP incumbent has no outgoing route arc".to_owned())
            })?;
        if route.contains(&next) || route.len() > input.problem.locations().len() {
            return Err(SolverError::Failed(
                "SCIP incumbent contains a cycle after MTZ validation".to_owned(),
            ));
        }
        route.push(next);
        current = next;
    }
    Ok(route)
}

#[cfg(test)]
mod tests;
