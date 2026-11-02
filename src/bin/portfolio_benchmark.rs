use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use troute::{
    api::{LocationInput, OptimizeRouteRequest},
    cancellation::CancellationToken,
    domain::{OptimizationProblem, StartPolicy, TimeOfDay, VisitPolicy},
    matrix::TravelTimeMatrix,
    solver::{
        PortfolioPolicy, PortfolioPolicyConfig, PortfolioTelemetrySummary, RouteSolver,
        SolverInput, SolverOrchestrator, SolverOrchestratorConfig, SolverRegistry, TelemetryMode,
    },
};

#[derive(Debug, Clone)]
struct BenchmarkConfig {
    output_dir: PathBuf,
    policies: Vec<PortfolioPolicy>,
    seeds: Vec<u64>,
    sizes: Vec<usize>,
    scenarios: Vec<String>,
    budgets_ms: Vec<u64>,
    slots: Vec<usize>,
}

#[derive(Debug, Serialize)]
struct Manifest {
    schema_version: u16,
    experiment_id: String,
    created_at_unix_ms: u128,
    git_sha: Option<String>,
    rust_version: Option<String>,
    os: &'static str,
    arch: &'static str,
    available_parallelism: Option<usize>,
    feature_flags: Vec<&'static str>,
    policies: Vec<PortfolioPolicy>,
    seeds: Vec<u64>,
    sizes: Vec<usize>,
    scenarios: Vec<String>,
    budgets_ms: Vec<u64>,
    slots: Vec<usize>,
    external_solver_availability: BTreeMap<String, Option<String>>,
}

#[derive(Debug, Serialize)]
struct RunRecord {
    schema_version: u16,
    experiment_id: String,
    run_index: usize,
    policy: PortfolioPolicy,
    seed: u64,
    size: usize,
    scenario: String,
    configured_budget_ms: u64,
    logical_cpu_slots: usize,
    problem_hash: String,
    matrix_hash: String,
    status: String,
    feasible: bool,
    elapsed_ms: u64,
    selected_strategy: Option<String>,
    objective: Option<troute::solver::ObjectiveVector>,
    proof_status: Option<String>,
    error: Option<String>,
    telemetry: Option<PortfolioTelemetrySummary>,
}

#[derive(Debug, Serialize)]
struct PolicySummary {
    policy: PortfolioPolicy,
    run_count: usize,
    completed_count: usize,
    feasible_count: usize,
    feasibility_rate: f64,
    wall_ms_p50: Option<u64>,
    wall_ms_p95: Option<u64>,
    proven_optimal_count: usize,
    decision_count: usize,
    nominal_reserved_slot_ms: u64,
    measured_cpu_rate: f64,
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_args(env::args().skip(1))?;
    let (experiment_id, directory) = create_experiment_directory(&config.output_dir)?;
    let default_solver = SolverOrchestratorConfig::default();
    let registry = SolverRegistry::built_in(&default_solver)?;
    let availability = registry
        .descriptors()
        .iter()
        .map(|descriptor| {
            (
                descriptor.id.to_string(),
                descriptor.unavailable_reason.clone(),
            )
        })
        .collect();
    let manifest = Manifest {
        schema_version: 1,
        experiment_id: experiment_id.clone(),
        created_at_unix_ms: SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
        git_sha: command_output("git", &["rev-parse", "HEAD"]),
        rust_version: command_output("rustc", &["--version"]),
        os: env::consts::OS,
        arch: env::consts::ARCH,
        available_parallelism: std::thread::available_parallelism().ok().map(usize::from),
        feature_flags: if cfg!(feature = "solver-mip-scip") {
            vec!["solver-mip-scip"]
        } else {
            Vec::new()
        },
        policies: config.policies.clone(),
        seeds: config.seeds.clone(),
        sizes: config.sizes.clone(),
        scenarios: config.scenarios.clone(),
        budgets_ms: config.budgets_ms.clone(),
        slots: config.slots.clone(),
        external_solver_availability: availability,
    };
    write_json(&directory.join("manifest.json"), &manifest)?;

    let mut records = Vec::new();
    let mut run_index = 0;
    for &size in &config.sizes {
        for scenario in &config.scenarios {
            for &seed in &config.seeds {
                let (request, matrix) = synthetic_instance(size, scenario, seed)?;
                let problem = OptimizationProblem::try_from(request)?;
                let problem_hash = problem_hash(&problem);
                let matrix_hash = matrix_hash(&matrix);
                for &budget_ms in &config.budgets_ms {
                    for &slots in &config.slots {
                        // Rotate policy order by instance and seed to reduce a
                        // consistent cache/thermal ordering bias.
                        let mut policies = config.policies.clone();
                        if !policies.is_empty() {
                            let policy_count = policies.len();
                            policies.rotate_left((seed as usize + size) % policy_count);
                        }
                        for policy in policies {
                            run_index += 1;
                            let safety_ms = (budget_ms / 20).clamp(1, 50);
                            let orchestrator = SolverOrchestrator::new(SolverOrchestratorConfig {
                                max_concurrency: slots,
                                strategy_timeout: Duration::from_millis(budget_ms.max(1)),
                                total_budget: Duration::from_millis(budget_ms.max(1)),
                                sa_chunk: Duration::from_millis((budget_ms / 4).clamp(1, 250)),
                                deadline_safety_margin: Duration::from_millis(
                                    safety_ms.min(budget_ms.saturating_sub(1).max(1)),
                                ),
                                sa_seeds: vec![seed],
                                compare_all: true,
                                portfolio: PortfolioPolicyConfig {
                                    policy,
                                    seed,
                                    min_quantum_ms: (budget_ms / 4).clamp(1, 250),
                                    ..PortfolioPolicyConfig::default()
                                },
                                telemetry_mode: TelemetryMode::Detailed,
                                ..SolverOrchestratorConfig::default()
                            });
                            let started = Instant::now();
                            let outcome = orchestrator.and_then(|solver| {
                                solver.solve_with_diagnostics(SolverInput {
                                    matrix: &matrix,
                                    problem: &problem,
                                    cancellation: &CancellationToken::new(),
                                })
                            });
                            let elapsed_ms = millis(started.elapsed());
                            let mut record = match outcome {
                                Ok(result) => {
                                    let diagnostics = result.diagnostics;
                                    let selected_strategy = diagnostics
                                        .as_ref()
                                        .map(|diagnostics| diagnostics.selected_strategy.clone());
                                    let selected = diagnostics.as_ref().and_then(|diagnostics| {
                                        diagnostics.candidates.iter().find(|candidate| {
                                            candidate.strategy == diagnostics.selected_strategy
                                                && candidate.feasible
                                        })
                                    });
                                    RunRecord {
                                        schema_version: 1,
                                        experiment_id: experiment_id.clone(),
                                        run_index,
                                        policy,
                                        seed,
                                        size,
                                        scenario: scenario.clone(),
                                        configured_budget_ms: budget_ms,
                                        logical_cpu_slots: slots,
                                        problem_hash: problem_hash.clone(),
                                        matrix_hash: matrix_hash.clone(),
                                        status: "COMPLETED".to_owned(),
                                        feasible: true,
                                        elapsed_ms,
                                        selected_strategy,
                                        objective: selected
                                            .and_then(|candidate| candidate.objective_score)
                                            .map(Into::into),
                                        proof_status: selected
                                            .and_then(|candidate| candidate.metadata.proof_status)
                                            .map(|proof| format!("{proof:?}").to_ascii_uppercase()),
                                        error: None,
                                        telemetry: diagnostics
                                            .and_then(|value| value.portfolio_telemetry),
                                    }
                                }
                                Err(error) => RunRecord {
                                    schema_version: 1,
                                    experiment_id: experiment_id.clone(),
                                    run_index,
                                    policy,
                                    seed,
                                    size,
                                    scenario: scenario.clone(),
                                    configured_budget_ms: budget_ms,
                                    logical_cpu_slots: slots,
                                    problem_hash: problem_hash.clone(),
                                    matrix_hash: matrix_hash.clone(),
                                    status: "FAILED".to_owned(),
                                    feasible: false,
                                    elapsed_ms,
                                    selected_strategy: None,
                                    objective: None,
                                    proof_status: None,
                                    error: Some(error.to_string()),
                                    telemetry: None,
                                },
                            };
                            if let Some(telemetry) = record.telemetry.as_mut() {
                                let telemetry_job_id =
                                    format!("{}-run-{}", experiment_id, run_index);
                                for event in &mut telemetry.events {
                                    event.job_id.clone_from(&telemetry_job_id);
                                }
                            }
                            append_json_line(&directory.join("runs.jsonl"), &record)?;
                            if let Some(telemetry) = &record.telemetry {
                                for event in &telemetry.events {
                                    append_json_line(&directory.join("timelines.jsonl"), event)?;
                                }
                            }
                            records.push(record);
                        }
                    }
                }
            }
        }
    }
    let summary = summarize(&records);
    write_json(&directory.join("summary.json"), &summary)?;
    write_summary_csv(&directory.join("summary.csv"), &summary)?;
    println!("portfolio benchmark completed: {}", directory.display());
    Ok(())
}

fn parse_args(arguments: impl Iterator<Item = String>) -> Result<BenchmarkConfig, String> {
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    let mut arguments = arguments.peekable();
    while let Some(argument) = arguments.next() {
        if argument == "--help" || argument == "-h" {
            return Err("usage: portfolio_benchmark [--output-dir DIR] [--policies CSV] [--seeds CSV] [--sizes CSV] [--scenarios CSV] [--budgets-ms CSV] [--slots CSV]".to_owned());
        }
        if !argument.starts_with("--") {
            return Err(format!("unexpected argument: {argument}"));
        }
        let value = arguments
            .next()
            .ok_or_else(|| format!("missing value for {argument}"))?;
        values.insert(argument, value);
    }
    let output_dir = values
        .remove("--output-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("benchmark-results"));
    let policies = values
        .remove("--policies")
        .map(|value| {
            parse_list(&value, |item| {
                item.parse::<PortfolioPolicy>()
                    .map_err(|error| error.to_string())
            })
        })
        .transpose()?
        .unwrap_or_else(|| PortfolioPolicy::all().to_vec());
    let seeds = numeric_option(&mut values, "--seeds", vec![0, 1])?;
    let sizes = numeric_option(&mut values, "--sizes", vec![5, 10])?;
    let budgets_ms = numeric_option(&mut values, "--budgets-ms", vec![1_000])?;
    let slots = numeric_option(&mut values, "--slots", vec![1, 2])?;
    let scenarios = values
        .remove("--scenarios")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| vec!["wide".to_owned(), "asymmetric".to_owned()]);
    const SCENARIOS: [&str; 8] = [
        "wide",
        "tight",
        "asymmetric",
        "sparse",
        "zero-arc",
        "partial",
        "mixed-priorities",
        "required-infeasible",
    ];
    if !values.is_empty()
        || policies.is_empty()
        || seeds.is_empty()
        || sizes.iter().any(|size| *size < 2 || *size > 500)
        || budgets_ms.iter().any(|budget| *budget < 2)
        || slots.iter().any(|slot| *slot == 0 || *slot > 256)
        || scenarios.is_empty()
        || scenarios
            .iter()
            .any(|scenario| !SCENARIOS.contains(&scenario.as_str()))
    {
        return Err("benchmark options are invalid or contain unknown flags".to_owned());
    }
    Ok(BenchmarkConfig {
        output_dir,
        policies,
        seeds,
        sizes,
        scenarios,
        budgets_ms,
        slots,
    })
}

fn numeric_option<T>(
    values: &mut BTreeMap<String, String>,
    name: &str,
    default: Vec<T>,
) -> Result<Vec<T>, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    values
        .remove(name)
        .map(|value| {
            parse_list(&value, |item| {
                item.parse::<T>().map_err(|error| error.to_string())
            })
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_list<T>(value: &str, parse: impl Fn(&str) -> Result<T, String>) -> Result<Vec<T>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(parse)
        .collect()
}

fn create_experiment_directory(root: &Path) -> Result<(String, PathBuf), Box<dyn Error>> {
    fs::create_dir_all(root)?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    for suffix in 0_u32..10_000 {
        let id = format!("portfolio-{timestamp}-{}-{suffix}", std::process::id());
        let directory = root.join(&id);
        match fs::create_dir(&directory) {
            Ok(()) => return Ok((id, directory)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err("could not allocate a unique experiment directory".into())
}

fn synthetic_instance(
    size: usize,
    scenario: &str,
    seed: u64,
) -> Result<(OptimizeRouteRequest, TravelTimeMatrix), Box<dyn Error>> {
    let open = TimeOfDay::from_minutes(0)?;
    let close = TimeOfDay::from_minutes(23 * 60 + 50)?;
    let partial = scenario == "partial" || scenario == "mixed-priorities";
    let locations = (0..size)
        .map(|index| LocationInput {
            id: format!("p{index}"),
            name: Some(format!("Synthetic {index}")),
            place_id: String::new(),
            open_time: open,
            close_time: close,
            stay_minutes: if scenario == "tight" && index > 0 {
                10
            } else {
                0
            },
            required: partial.then_some(index == 0 || index + 1 == size),
            priority: partial.then_some(((index * 17 + seed as usize) % 100 + 1) as u8),
        })
        .collect();
    let mut rows = vec![vec![None; size]; size];
    for (from, row) in rows.iter_mut().enumerate() {
        for (to, value) in row.iter_mut().enumerate() {
            *value = if from == to {
                Some(0)
            } else if (scenario == "sparse" && to != from + 1 && !(from == 0 && to + 1 == size))
                || (scenario == "required-infeasible" && (from == size / 2 || to == size / 2))
            {
                None
            } else if scenario == "zero-arc" && (from + 1) % size == to {
                Some(0)
            } else {
                let symmetric = from.abs_diff(to) as u64 * 7 + 1;
                let directed = if scenario == "wide" {
                    0
                } else {
                    (from * 13 + to * 5) as u64
                };
                Some(((symmetric + directed + seed) % 45 + 1) as u32)
            };
        }
    }
    let matrix = TravelTimeMatrix::new_sparse(rows.clone())?;
    let request = OptimizeRouteRequest {
        job_id: format!("benchmark-{scenario}-{size}-{seed}"),
        locations,
        visit_policy: partial.then_some(VisitPolicy::BestEffortOnFailure),
        start_policy: Some(StartPolicy::Fixed),
        start_time: Some(TimeOfDay::from_minutes(9 * 60)?),
        travel_date: None,
        time_zone: None,
        travel_mode: None,
        country_code: None,
        route_provider: None,
        travel_time_matrix: None,
        sparse_travel_time_matrix: Some(rows),
        debug: None,
        portfolio_experiment: None,
    };
    Ok((request, matrix))
}

fn summarize(records: &[RunRecord]) -> Vec<PolicySummary> {
    PortfolioPolicy::all()
        .iter()
        .filter_map(|policy| {
            let policy_records: Vec<_> = records
                .iter()
                .filter(|record| record.policy == *policy)
                .collect();
            if policy_records.is_empty() {
                return None;
            }
            let mut walls: Vec<_> = policy_records
                .iter()
                .map(|record| record.elapsed_ms)
                .collect();
            walls.sort_unstable();
            let telemetry: Vec<_> = policy_records
                .iter()
                .filter_map(|record| record.telemetry.as_ref())
                .collect();
            let run_metrics: Vec<_> = telemetry.iter().flat_map(|summary| &summary.runs).collect();
            let measured_cpu = run_metrics
                .iter()
                .filter(|run| run.cpu_ms.is_some())
                .count();
            Some(PolicySummary {
                policy: *policy,
                run_count: policy_records.len(),
                completed_count: policy_records
                    .iter()
                    .filter(|record| record.status == "COMPLETED")
                    .count(),
                feasible_count: policy_records
                    .iter()
                    .filter(|record| record.feasible)
                    .count(),
                feasibility_rate: policy_records
                    .iter()
                    .filter(|record| record.feasible)
                    .count() as f64
                    / policy_records.len() as f64,
                wall_ms_p50: percentile(&walls, 0.5),
                wall_ms_p95: percentile(&walls, 0.95),
                proven_optimal_count: policy_records
                    .iter()
                    .filter(|record| record.proof_status.as_deref() == Some("OPTIMAL"))
                    .count(),
                decision_count: telemetry
                    .iter()
                    .flat_map(|summary| &summary.events)
                    .filter(|event| {
                        event.event_type == troute::solver::TelemetryEventType::SchedulerDecision
                    })
                    .count(),
                nominal_reserved_slot_ms: run_metrics.iter().map(|run| run.reserved_slot_ms).sum(),
                measured_cpu_rate: if run_metrics.is_empty() {
                    0.0
                } else {
                    measured_cpu as f64 / run_metrics.len() as f64
                },
            })
        })
        .collect()
}

fn percentile(values: &[u64], quantile: f64) -> Option<u64> {
    (!values.is_empty()).then(|| values[((values.len() - 1) as f64 * quantile).floor() as usize])
}

fn problem_hash(problem: &OptimizationProblem) -> String {
    let mut bytes = Vec::new();
    for location in problem.locations() {
        bytes.extend_from_slice(location.id().as_bytes());
        bytes.extend_from_slice(&location.stay_minutes().to_le_bytes());
        bytes.push(location.required() as u8);
        bytes.push(location.priority());
    }
    stable_hash(&bytes)
}

fn matrix_hash(matrix: &TravelTimeMatrix) -> String {
    let mut bytes = Vec::new();
    for from in 0..matrix.size() {
        for to in 0..matrix.size() {
            bytes.extend_from_slice(
                &matrix
                    .travel_minutes(from, to)
                    .unwrap_or(u32::MAX)
                    .to_le_bytes(),
            );
        }
    }
    stable_hash(&bytes)
}

fn stable_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    let file = File::create(path)?;
    serde_json::to_writer_pretty(BufWriter::new(file), value)?;
    Ok(())
}

fn append_json_line(path: &Path, value: &impl Serialize) -> Result<(), Box<dyn Error>> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn write_summary_csv(path: &Path, summaries: &[PolicySummary]) -> Result<(), Box<dyn Error>> {
    let mut writer = BufWriter::new(File::create(path)?);
    writeln!(writer, "policy,run_count,completed_count,feasible_count,feasibility_rate,wall_ms_p50,wall_ms_p95,proven_optimal_count,decision_count,nominal_reserved_slot_ms,measured_cpu_rate")?;
    for summary in summaries {
        writeln!(
            writer,
            "{},{},{},{},{:.6},{},{},{},{},{},{:.6}",
            summary.policy,
            summary.run_count,
            summary.completed_count,
            summary.feasible_count,
            summary.feasibility_rate,
            summary
                .wall_ms_p50
                .map_or_else(String::new, |value| value.to_string()),
            summary
                .wall_ms_p95
                .map_or_else(String::new, |value| value.to_string()),
            summary.proven_optimal_count,
            summary.decision_count,
            summary.nominal_reserved_slot_ms,
            summary.measured_cpu_rate
        )?;
    }
    Ok(())
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    Command::new(program)
        .args(arguments)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
