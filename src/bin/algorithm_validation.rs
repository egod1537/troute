use std::{
    cmp::Ordering,
    env,
    error::Error,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use rand::{rngs::StdRng, Rng, SeedableRng};
use serde::Serialize;
use troute::{
    cancellation::CancellationToken,
    domain::OptimizationProblem,
    matrix::TravelTimeMatrix,
    solver::{
        evaluate_solution, BitDpPerfectMatching, BlossomPerfectMatching, ChristofidesInitialRoute,
        ClusteredMstGreedyInitialRoute, ClusteredSolver, ClusteredSolverConfig,
        DefaultObjectivePolicy, ExactBitDpSolver, FrontierPoint, FrontierPolicy,
        GreedyInitialRoute, InitialRouteGenerator, MstDoubleTreeInitialRoute,
        PerfectMatchingStrategy, SimulatedAnnealingConfig, SimulatedAnnealingSolver,
        SolutionMetrics, SolverCandidate, SolverInput, SolverOrchestrator,
        SolverOrchestratorConfig, SolverSolution, TIME_SLOT_MINUTES,
    },
    OptimizeRouteRequest,
};

const SMALL_N: [usize; 5] = [5, 8, 10, 12, 15];
const LARGE_N: [usize; 3] = [20, 30, 50];
const BRUTE_FORCE_N: [usize; 3] = [5, 8, 10];
const DEFAULT_SEEDS: [u64; 2] = [42, 1_337];

#[derive(Debug)]
struct Config {
    output_dir: PathBuf,
    sizes: Option<Vec<usize>>,
    seeds: Vec<u64>,
    budgets: Vec<Duration>,
    enable_mip_scip: bool,
    lkh3_executable: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum QualityScenario {
    DirectedTimeWindows,
    SparseDirected,
    ZeroMinuteArcs,
    PartialVisit,
    Infeasible,
}

#[derive(Debug, Serialize)]
struct BenchmarkManifest {
    generated_at: String,
    commit_sha: Option<String>,
    rustc_version: Option<String>,
    target_arch: &'static str,
    target_os: &'static str,
    sizes: Vec<usize>,
    seeds: Vec<u64>,
    budgets_ms: Vec<u64>,
    scenarios: Vec<QualityScenario>,
    mip_scip_requested: bool,
    mip_scip_compiled: bool,
    lkh3_executable: Option<PathBuf>,
    resource_accounting: &'static str,
}

#[derive(Debug, Serialize)]
struct QualityBenchmarkRecord {
    scenario: QualityScenario,
    n: usize,
    seed: u64,
    budget_ms: u64,
    strategy: String,
    selected: bool,
    feasible: bool,
    visited_optional_count: Option<usize>,
    visited_optional_priority: Option<u32>,
    selected_start_minute: Option<u16>,
    finish_minute: Option<u16>,
    travel_minutes: Option<u32>,
    wait_minutes: Option<u32>,
    elapsed_ms: f64,
    candidate_attempts: usize,
    first_feasible_ms: Option<f64>,
    outcome: Option<String>,
    proof_status: Option<String>,
    best_bound: Option<u64>,
    relative_gap_ppm: Option<u64>,
    thread_count: Option<usize>,
    source_version: Option<String>,
    timed_out: bool,
    error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct MetricsRecord {
    objective: u64,
    latest_start: u32,
    finish: u32,
    travel: u32,
    wait: u32,
}

impl From<SolutionMetrics> for MetricsRecord {
    fn from(value: SolutionMetrics) -> Self {
        Self {
            objective: value.score,
            latest_start: u32::from(value.start_time_slot) * TIME_SLOT_MINUTES,
            finish: u32::from(value.finish_time_slot) * TIME_SLOT_MINUTES,
            travel: value.travel_minutes,
            wait: value.wait_minutes,
        }
    }
}

#[derive(Debug, Serialize)]
struct ExactCheckRecord {
    n: usize,
    seed: u64,
    passed: bool,
    feasible: bool,
    exact: Option<MetricsRecord>,
    brute_force: Option<MetricsRecord>,
    generated_states: Option<usize>,
    frontier_states: Option<usize>,
    unpruned_generated_states: Option<usize>,
    unpruned_frontier_states: Option<usize>,
    pruning_ratio: Option<f64>,
    elapsed_ms: f64,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct BenchmarkRecord {
    n: usize,
    seed: u64,
    strategy: String,
    feasible: bool,
    objective: Option<u64>,
    latest_start: Option<u32>,
    finish: Option<u32>,
    travel: Option<u32>,
    wait: Option<u32>,
    elapsed_ms: f64,
    generated_states: Option<usize>,
    frontier_states: Option<usize>,
    cluster_count: Option<usize>,
    iteration_count: Option<u64>,
    accepted_moves: Option<u64>,
    improved_moves: Option<u64>,
    initial_score: Option<u64>,
    objective_gap: Option<i128>,
    latest_start_gap: Option<i64>,
    finish_gap: Option<i64>,
    travel_gap: Option<i64>,
    wait_gap: Option<i64>,
    selected: bool,
    timed_out: bool,
    error: Option<String>,
}

struct GeneratedCase {
    problem: OptimizationProblem,
    matrix: TravelTimeMatrix,
}

#[derive(Clone, Copy)]
struct NoPruning;

impl FrontierPolicy for NoPruning {
    fn dominates(&self, _left: FrontierPoint, _right: FrontierPoint) -> bool {
        false
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let config = parse_args()?;
    fs::create_dir_all(&config.output_dir)?;

    let brute_sizes = selected_sizes(&config, &BRUTE_FORCE_N, 2, 10);
    let small_sizes = selected_sizes(&config, &SMALL_N, 2, 15);
    let large_sizes = selected_sizes(&config, &LARGE_N, 16, usize::MAX);
    let all_sizes = selected_sizes(&config, &[5, 8, 10, 12, 15, 20, 30, 50], 2, usize::MAX);

    let exact = exact_checks(&brute_sizes, &config.seeds);
    let exact_failed = exact.iter().any(|record| !record.passed);
    write_json(&config.output_dir.join("exact-vs-bruteforce.json"), &exact)?;

    let heuristic = heuristic_benchmarks(&small_sizes, &config.seeds);
    write_json(&config.output_dir.join("heuristic-gap.json"), &heuristic)?;

    let large = large_benchmarks(&large_sizes, &config.seeds);
    write_json(&config.output_dir.join("large-n-performance.json"), &large)?;

    let orchestrator = orchestrator_benchmarks(&all_sizes, &config.seeds);
    write_json(
        &config.output_dir.join("orchestrator-results.json"),
        &orchestrator,
    )?;

    let quality = quality_benchmarks(&all_sizes, &config)?;
    write_json(
        &config.output_dir.join("solver-quality-results.json"),
        &quality,
    )?;
    let scenarios = quality_scenarios();
    let manifest = BenchmarkManifest {
        generated_at: chrono::Utc::now().to_rfc3339(),
        commit_sha: command_output("git", &["rev-parse", "HEAD"]),
        rustc_version: command_output("rustc", &["--version"]),
        target_arch: std::env::consts::ARCH,
        target_os: std::env::consts::OS,
        sizes: all_sizes.clone(),
        seeds: config.seeds.clone(),
        budgets_ms: config
            .budgets
            .iter()
            .map(|budget| duration_ms(*budget))
            .collect(),
        scenarios,
        mip_scip_requested: config.enable_mip_scip,
        mip_scip_compiled: cfg!(feature = "solver-mip-scip"),
        lkh3_executable: config.lkh3_executable.clone(),
        resource_accounting: "wall time and configured solver thread count are recorded; process CPU and peak RSS require an external benchmark harness",
    };
    write_json(
        &config.output_dir.join("benchmark-manifest.json"),
        &manifest,
    )?;
    write_quality_report(
        &config.output_dir.join("solver-quality-report.md"),
        &quality,
    )?;

    cross_check_matching(&config.seeds)?;
    println!("validation results: {}", config.output_dir.display());
    if exact_failed {
        return Err("Exact/brute-force mismatch; inspect the recorded n and seed".into());
    }
    Ok(())
}

fn parse_args() -> Result<Config, Box<dyn Error>> {
    let mut output_dir = PathBuf::from("benchmark-results");
    let mut sizes = None;
    let mut seeds = DEFAULT_SEEDS.to_vec();
    let mut budgets = vec![
        Duration::from_secs(1),
        Duration::from_secs(5),
        Duration::from_secs(30),
    ];
    let mut enable_mip_scip = false;
    let mut lkh3_executable = None;
    let args: Vec<_> = env::args().skip(1).collect();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--output-dir" => {
                index += 1;
                output_dir = args
                    .get(index)
                    .ok_or("--output-dir requires a path")?
                    .into();
            }
            "--n" => {
                index += 1;
                let parsed = parse_csv(args.get(index).ok_or("--n requires values")?)?;
                if parsed.iter().any(|&n| n < 2) {
                    return Err("--n values must be at least 2".into());
                }
                sizes = Some(parsed.into_iter().map(|value| value as usize).collect());
            }
            "--seed" => {
                index += 1;
                seeds = parse_csv(args.get(index).ok_or("--seed requires values")?)?;
            }
            "--budget-seconds" => {
                index += 1;
                let values = parse_csv(args.get(index).ok_or("--budget-seconds requires values")?)?;
                if values.is_empty() || values.contains(&0) {
                    return Err("--budget-seconds values must be positive".into());
                }
                budgets = values.into_iter().map(Duration::from_secs).collect();
            }
            "--enable-mip" => enable_mip_scip = true,
            "--lkh-executable" => {
                index += 1;
                lkh3_executable = Some(
                    args.get(index)
                        .ok_or("--lkh-executable requires a path")?
                        .into(),
                );
            }
            "--help" | "-h" => {
                println!(
                    "Usage: algorithm_validation [--output-dir DIR] [--n 5,8,10] [--seed 42,1337] [--budget-seconds 1,5,30] [--enable-mip] [--lkh-executable PATH]"
                );
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument: {unknown}").into()),
        }
        index += 1;
    }
    if seeds.is_empty() {
        return Err("at least one seed is required".into());
    }
    Ok(Config {
        output_dir,
        sizes,
        seeds,
        budgets,
        enable_mip_scip,
        lkh3_executable,
    })
}

fn parse_csv(value: &str) -> Result<Vec<u64>, Box<dyn Error>> {
    value
        .split(',')
        .map(|part| part.parse::<u64>().map_err(Into::into))
        .collect()
}

fn selected_sizes(
    config: &Config,
    defaults: &[usize],
    minimum: usize,
    maximum: usize,
) -> Vec<usize> {
    config
        .sizes
        .as_ref()
        .map(|sizes| {
            sizes
                .iter()
                .copied()
                .filter(|size| (minimum..=maximum).contains(size))
                .collect()
        })
        .unwrap_or_else(|| defaults.to_vec())
}

fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<(), Box<dyn Error>> {
    let writer = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(writer, value)?;
    Ok(())
}

fn elapsed_ms(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1_000.0
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn command_output(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn quality_scenarios() -> Vec<QualityScenario> {
    vec![
        QualityScenario::DirectedTimeWindows,
        QualityScenario::SparseDirected,
        QualityScenario::ZeroMinuteArcs,
        QualityScenario::PartialVisit,
        QualityScenario::Infeasible,
    ]
}

fn generated_case(n: usize, seed: u64) -> GeneratedCase {
    let mut rng = StdRng::seed_from_u64(seed ^ ((n as u64) << 32));
    let locations: Vec<_> = (0..n)
        .map(|index| {
            let endpoint = index == 0 || index + 1 == n;
            let open = if endpoint {
                0
            } else {
                rng.gen_range(8 * 6..=11 * 6) * 10
            };
            serde_json::json!({
                "id": format!("location-{index}"),
                "place_id": format!("place-{index}"),
                "open_time": hhmm(open),
                "close_time": "23:50",
                "stay_minutes": if endpoint { 0 } else { rng.gen_range(0..=1) * 10 },
            })
        })
        .collect();
    let request: OptimizeRouteRequest = serde_json::from_value(serde_json::json!({
        "job_id": format!("algorithm-benchmark-{n}-{seed}"),
        "locations": locations,
        "start_time": "08:00",
    }))
    .expect("generated requests are valid");
    let matrix = TravelTimeMatrix::new(
        (0..n)
            .map(|from| {
                (0..n)
                    .map(|to| if from == to { 0 } else { rng.gen_range(1..=12) })
                    .collect()
            })
            .collect(),
    )
    .expect("generated matrices are square");
    GeneratedCase {
        problem: request.try_into().expect("generated problem is valid"),
        matrix,
    }
}

fn generated_quality_case(n: usize, seed: u64, scenario: QualityScenario) -> GeneratedCase {
    let mut rng = StdRng::seed_from_u64(seed ^ ((n as u64) << 32) ^ scenario as u64);
    let locations: Vec<_> = (0..n)
        .map(|index| {
            let endpoint = index == 0 || index + 1 == n;
            let (open, close, stay) = match scenario {
                QualityScenario::DirectedTimeWindows => {
                    if endpoint {
                        (0, 23 * 60 + 50, 0)
                    } else {
                        let open = rng.gen_range(8 * 6..=11 * 6) * 10;
                        (open, 23 * 60 + 50, rng.gen_range(0..=2) * 10)
                    }
                }
                QualityScenario::Infeasible if index == 1 => (0, 0, 0),
                _ => (0, 23 * 60 + 50, if endpoint { 0 } else { 10 }),
            };
            let mut value = serde_json::json!({
                "id": format!("location-{index}"),
                "place_id": format!("place-{index}"),
                "open_time": hhmm(open),
                "close_time": hhmm(close),
                "stay_minutes": stay,
            });
            if matches!(scenario, QualityScenario::PartialVisit) {
                value["required"] = serde_json::json!(endpoint || index % 4 == 0);
                value["priority"] = serde_json::json!(1 + ((index * 17 + seed as usize) % 100));
            }
            value
        })
        .collect();
    let mut request = serde_json::json!({
        "job_id": format!("quality-{scenario:?}-{n}-{seed}"),
        "locations": locations,
        "start_policy": if seed.is_multiple_of(2) { "FIXED" } else { "LATEST" },
        "start_time": "08:00",
    });
    if matches!(scenario, QualityScenario::PartialVisit) {
        request["visit_policy"] = serde_json::json!("BEST_EFFORT_ON_FAILURE");
    }
    let request: OptimizeRouteRequest =
        serde_json::from_value(request).expect("generated quality request is valid");

    let mut rows = vec![vec![None; n]; n];
    for (index, row) in rows.iter_mut().enumerate() {
        row[index] = Some(0);
    }
    for (from, row) in rows.iter_mut().enumerate() {
        for (to, cell) in row.iter_mut().enumerate() {
            if from == to {
                continue;
            }
            let base = rng.gen_range(1..=45);
            *cell = match scenario {
                QualityScenario::SparseDirected if rng.gen_bool(0.55) => None,
                QualityScenario::ZeroMinuteArcs
                    if (from + to + seed as usize).is_multiple_of(7) =>
                {
                    Some(0)
                }
                QualityScenario::Infeasible if from == 0 => None,
                _ => Some(base),
            };
        }
    }
    if matches!(scenario, QualityScenario::SparseDirected) {
        for index in 0..n - 1 {
            rows[index][index + 1] = Some(1 + (index as u32 % 9));
        }
    }
    let matrix = TravelTimeMatrix::new_sparse(rows).expect("generated matrix is square");
    GeneratedCase {
        problem: request.try_into().expect("generated problem is valid"),
        matrix,
    }
}

fn input<'a>(case: &'a GeneratedCase, cancellation: &'a CancellationToken) -> SolverInput<'a> {
    SolverInput {
        matrix: &case.matrix,
        problem: &case.problem,
        cancellation,
    }
}

fn exact_checks(sizes: &[usize], seeds: &[u64]) -> Vec<ExactCheckRecord> {
    let mut records = Vec::new();
    for &n in sizes {
        for &seed in seeds {
            let case = generated_case(n, seed);
            let cancellation = CancellationToken::new();
            let started = Instant::now();
            let exact = ExactBitDpSolver::default().solve_detailed(input(&case, &cancellation));
            let unpruned = ExactBitDpSolver::new(DefaultObjectivePolicy, NoPruning)
                .solve_detailed(input(&case, &cancellation));
            let brute = brute_force(&case);
            let elapsed_ms = elapsed_ms(started.elapsed());
            let (exact_metrics, generated, frontier, exact_error) = match &exact {
                Ok(result) => (
                    Some(result.metrics.into()),
                    Some(result.stats.generated_states),
                    Some(result.stats.frontier_states),
                    None,
                ),
                Err(error) => (None, None, None, Some(error.to_string())),
            };
            let (unpruned_generated, unpruned_frontier) = match &unpruned {
                Ok(result) => (
                    Some(result.stats.generated_states),
                    Some(result.stats.frontier_states),
                ),
                Err(_) => (None, None),
            };
            let brute_metrics = brute.as_ref().map(|(_, metrics)| (*metrics).into());
            let passed = exact_metrics == brute_metrics
                && match (&exact, &unpruned) {
                    (Ok(left), Ok(right)) => left.metrics == right.metrics,
                    (Err(_), Err(_)) => true,
                    _ => false,
                };
            let pruning_ratio = frontier
                .zip(unpruned_frontier)
                .and_then(|(left, right)| (right != 0).then_some(1.0 - left as f64 / right as f64));
            let error = (!passed).then(|| {
                format!(
                    "n={n}, seed={seed}: exact={exact_metrics:?}, brute={brute_metrics:?}, exact_error={exact_error:?}"
                )
            });
            records.push(ExactCheckRecord {
                n,
                seed,
                passed,
                feasible: exact_metrics.is_some(),
                exact: exact_metrics,
                brute_force: brute_metrics,
                generated_states: generated,
                frontier_states: frontier,
                unpruned_generated_states: unpruned_generated,
                unpruned_frontier_states: unpruned_frontier,
                pruning_ratio,
                elapsed_ms,
                error,
            });
        }
    }
    records
}

fn heuristic_benchmarks(sizes: &[usize], seeds: &[u64]) -> Vec<BenchmarkRecord> {
    let mut records = Vec::new();
    for &n in sizes {
        for &seed in seeds {
            let case = generated_case(n, seed);
            let cancellation = CancellationToken::new();
            let (exact, optimum) = timed_exact(&case, &cancellation, seed);
            records.push(exact);

            let started = Instant::now();
            let clustered = ClusteredSolver::with_config(ClusteredSolverConfig::new(5).unwrap())
                .solve_detailed(input(&case, &cancellation));
            records.push(match clustered {
                Ok(result) => row_from_metrics(
                    n,
                    seed,
                    "clustered",
                    result.metrics,
                    started.elapsed(),
                    Some(result.stats.exact_generated_states),
                    Some(result.stats.exact_frontier_states),
                    Some(result.stats.cluster_count),
                    None,
                    None,
                    None,
                    optimum,
                ),
                Err(error) => {
                    failed_row(n, seed, "clustered", started.elapsed(), error.to_string())
                }
            });

            records.push(timed_generator(
                "greedy",
                GreedyInitialRoute,
                &case,
                &cancellation,
                seed,
                optimum,
            ));
            records.push(timed_generator(
                "mst_double_tree",
                MstDoubleTreeInitialRoute::default(),
                &case,
                &cancellation,
                seed,
                optimum,
            ));
            records.push(timed_generator(
                "christofides",
                ChristofidesInitialRoute::default(),
                &case,
                &cancellation,
                seed,
                optimum,
            ));
            records.push(timed_annealing(&case, &cancellation, seed, optimum, 3_000));
        }
    }
    records
}

fn large_benchmarks(sizes: &[usize], seeds: &[u64]) -> Vec<BenchmarkRecord> {
    let mut records = Vec::new();
    for &n in sizes {
        for &seed in seeds {
            let case = generated_case(n, seed);
            let cancellation = CancellationToken::new();
            let started = Instant::now();
            let clustered = ClusteredSolver::default().solve_detailed(input(&case, &cancellation));
            records.push(match clustered {
                Ok(result) => row_from_metrics(
                    n,
                    seed,
                    "clustered",
                    result.metrics,
                    started.elapsed(),
                    Some(result.stats.exact_generated_states),
                    Some(result.stats.exact_frontier_states),
                    Some(result.stats.cluster_count),
                    None,
                    None,
                    None,
                    None,
                ),
                Err(error) => {
                    failed_row(n, seed, "clustered", started.elapsed(), error.to_string())
                }
            });
            records.push(timed_generator(
                "greedy",
                GreedyInitialRoute,
                &case,
                &cancellation,
                seed,
                None,
            ));
            records.push(timed_generator(
                "mst_double_tree",
                MstDoubleTreeInitialRoute::default(),
                &case,
                &cancellation,
                seed,
                None,
            ));
            records.push(timed_generator(
                "christofides",
                ChristofidesInitialRoute::default(),
                &case,
                &cancellation,
                seed,
                None,
            ));
            records.push(timed_annealing(&case, &cancellation, seed, None, 5_000));
        }
    }
    records
}

fn timed_exact(
    case: &GeneratedCase,
    cancellation: &CancellationToken,
    seed: u64,
) -> (BenchmarkRecord, Option<SolutionMetrics>) {
    let started = Instant::now();
    match ExactBitDpSolver::default().solve_detailed(input(case, cancellation)) {
        Ok(result) => (
            row_from_metrics(
                case.problem.locations().len(),
                seed,
                "exact_bit_dp",
                result.metrics,
                started.elapsed(),
                Some(result.stats.generated_states),
                Some(result.stats.frontier_states),
                None,
                None,
                None,
                None,
                Some(result.metrics),
            ),
            Some(result.metrics),
        ),
        Err(error) => (
            failed_row(
                case.problem.locations().len(),
                seed,
                "exact_bit_dp",
                started.elapsed(),
                error.to_string(),
            ),
            None,
        ),
    }
}

fn timed_generator<G: InitialRouteGenerator>(
    name: &str,
    generator: G,
    case: &GeneratedCase,
    cancellation: &CancellationToken,
    seed: u64,
    optimum: Option<SolutionMetrics>,
) -> BenchmarkRecord {
    let started = Instant::now();
    let mut rng = StdRng::seed_from_u64(seed);
    let generated = generator.generate(input(case, cancellation), &mut rng);
    match generated.and_then(|solution| {
        evaluate_solution(
            &DefaultObjectivePolicy,
            input(case, cancellation),
            &solution,
        )
    }) {
        Ok(metrics) => row_from_metrics(
            case.problem.locations().len(),
            seed,
            name,
            metrics,
            started.elapsed(),
            None,
            None,
            None,
            None,
            None,
            None,
            optimum,
        ),
        Err(error) => failed_row(
            case.problem.locations().len(),
            seed,
            name,
            started.elapsed(),
            error.to_string(),
        ),
    }
}

fn timed_annealing(
    case: &GeneratedCase,
    cancellation: &CancellationToken,
    seed: u64,
    optimum: Option<SolutionMetrics>,
    iterations: u64,
) -> BenchmarkRecord {
    let mut initial_rng = StdRng::seed_from_u64(seed);
    let initial_score = ClusteredMstGreedyInitialRoute::default()
        .generate(input(case, cancellation), &mut initial_rng)
        .ok()
        .and_then(|solution| {
            evaluate_solution(
                &DefaultObjectivePolicy,
                input(case, cancellation),
                &solution,
            )
            .ok()
        })
        .map(|metrics| metrics.score);
    let solver = SimulatedAnnealingSolver::with_config(SimulatedAnnealingConfig {
        iteration_limit: Some(iterations),
        seed: Some(seed),
        ..SimulatedAnnealingConfig::default()
    })
    .unwrap();
    let started = Instant::now();
    match solver.solve_detailed(input(case, cancellation)) {
        Ok(result) => row_from_metrics(
            case.problem.locations().len(),
            seed,
            "simulated_annealing",
            result.metrics,
            started.elapsed(),
            None,
            None,
            None,
            Some(result.stats.iterations),
            Some(result.stats.accepted_moves),
            Some(result.stats.improved_moves),
            optimum,
        )
        .with_initial_score(initial_score),
        Err(error) => failed_row(
            case.problem.locations().len(),
            seed,
            "simulated_annealing",
            started.elapsed(),
            error.to_string(),
        )
        .with_initial_score(initial_score),
    }
}

#[allow(clippy::too_many_arguments)]
fn row_from_metrics(
    n: usize,
    seed: u64,
    strategy: &str,
    metrics: SolutionMetrics,
    elapsed: Duration,
    generated_states: Option<usize>,
    frontier_states: Option<usize>,
    cluster_count: Option<usize>,
    iteration_count: Option<u64>,
    accepted_moves: Option<u64>,
    improved_moves: Option<u64>,
    optimum: Option<SolutionMetrics>,
) -> BenchmarkRecord {
    let metrics_record: MetricsRecord = metrics.into();
    BenchmarkRecord {
        n,
        seed,
        strategy: strategy.to_owned(),
        feasible: true,
        objective: Some(metrics_record.objective),
        latest_start: Some(metrics_record.latest_start),
        finish: Some(metrics_record.finish),
        travel: Some(metrics_record.travel),
        wait: Some(metrics_record.wait),
        elapsed_ms: elapsed_ms(elapsed),
        generated_states,
        frontier_states,
        cluster_count,
        iteration_count,
        accepted_moves,
        improved_moves,
        initial_score: None,
        objective_gap: optimum.map(|value| i128::from(metrics.score) - i128::from(value.score)),
        latest_start_gap: optimum.map(|value| {
            (i64::from(value.start_time_slot) - i64::from(metrics.start_time_slot))
                * i64::from(TIME_SLOT_MINUTES)
        }),
        finish_gap: optimum.map(|value| {
            (i64::from(metrics.finish_time_slot) - i64::from(value.finish_time_slot))
                * i64::from(TIME_SLOT_MINUTES)
        }),
        travel_gap: optimum
            .map(|value| i64::from(metrics.travel_minutes) - i64::from(value.travel_minutes)),
        wait_gap: optimum
            .map(|value| i64::from(metrics.wait_minutes) - i64::from(value.wait_minutes)),
        selected: false,
        timed_out: false,
        error: None,
    }
}

fn failed_row(
    n: usize,
    seed: u64,
    strategy: &str,
    elapsed: Duration,
    error: String,
) -> BenchmarkRecord {
    BenchmarkRecord {
        n,
        seed,
        strategy: strategy.to_owned(),
        feasible: false,
        objective: None,
        latest_start: None,
        finish: None,
        travel: None,
        wait: None,
        elapsed_ms: elapsed_ms(elapsed),
        generated_states: None,
        frontier_states: None,
        cluster_count: None,
        iteration_count: None,
        accepted_moves: None,
        improved_moves: None,
        initial_score: None,
        objective_gap: None,
        latest_start_gap: None,
        finish_gap: None,
        travel_gap: None,
        wait_gap: None,
        selected: false,
        timed_out: false,
        error: Some(error),
    }
}

impl BenchmarkRecord {
    fn with_initial_score(mut self, initial_score: Option<u64>) -> Self {
        self.initial_score = initial_score;
        self
    }
}

fn orchestrator_benchmarks(sizes: &[usize], seeds: &[u64]) -> Vec<BenchmarkRecord> {
    let mut records = Vec::new();
    for &n in sizes {
        for &seed in seeds {
            let case = generated_case(n, seed);
            let cancellation = CancellationToken::new();
            let solver = SolverOrchestrator::new(SolverOrchestratorConfig {
                max_cluster_size: 5,
                strategy_timeout: Duration::from_secs(10),
                sa_config: SimulatedAnnealingConfig {
                    iteration_limit: Some(if n <= 15 { 3_000 } else { 5_000 }),
                    ..SimulatedAnnealingConfig::default()
                },
                sa_seeds: vec![seed],
                ..SolverOrchestratorConfig::default()
            })
            .unwrap();
            let result = solver.solve_detailed(input(&case, &cancellation));
            match result {
                Ok(result) => {
                    for candidate in result.candidates {
                        records.push(candidate_row(n, seed, candidate, &result.selected_strategy));
                    }
                }
                Err(error) => records.push(failed_row(
                    n,
                    seed,
                    "orchestrator",
                    Duration::ZERO,
                    error.to_string(),
                )),
            }
        }
    }
    records
}

fn quality_benchmarks(
    sizes: &[usize],
    config: &Config,
) -> Result<Vec<QualityBenchmarkRecord>, Box<dyn Error>> {
    let mut records = Vec::new();
    for scenario in quality_scenarios() {
        for &n in sizes {
            for &seed in &config.seeds {
                let case = generated_quality_case(n, seed, scenario);
                for &budget in &config.budgets {
                    let cancellation = CancellationToken::new();
                    let safety = Duration::from_millis(50).min(budget / 10);
                    let solver = SolverOrchestrator::new(SolverOrchestratorConfig {
                        max_cluster_size: 5,
                        max_concurrency: 4,
                        strategy_timeout: budget,
                        total_budget: budget,
                        sa_chunk: Duration::from_millis(250).min(budget / 2),
                        deadline_safety_margin: safety.max(Duration::from_millis(1)),
                        sa_config: SimulatedAnnealingConfig {
                            seed: Some(seed),
                            ..SimulatedAnnealingConfig::default()
                        },
                        sa_seeds: vec![seed],
                        enable_mip_scip: config.enable_mip_scip,
                        mip_scip_time_limit: budget,
                        enable_lkh3: config.lkh3_executable.is_some(),
                        lkh3_executable: config.lkh3_executable.clone(),
                        lkh3_time_limit: budget,
                        lkh3_seed: seed,
                        compare_all: true,
                        ..SolverOrchestratorConfig::default()
                    })?;
                    match solver.solve_detailed(input(&case, &cancellation)) {
                        Ok(result) => {
                            let selected = result.selected_strategy;
                            let candidate_attempts = result.candidates.len();
                            let first_feasible_ms = result
                                .candidates
                                .iter()
                                .filter(|candidate| candidate.feasible)
                                .map(|candidate| elapsed_ms(candidate.elapsed))
                                .min_by(f64::total_cmp);
                            let first_sa = result
                                .candidates
                                .iter()
                                .position(|candidate| candidate.strategy.starts_with("sa_"));
                            records.extend(result.candidates.into_iter().enumerate().filter_map(
                                |(index, candidate)| {
                                    let keep = !candidate.strategy.starts_with("sa_")
                                        || candidate.strategy == selected
                                        || Some(index) == first_sa;
                                    keep.then(|| {
                                        quality_candidate_row(
                                            QualityRunContext {
                                                scenario,
                                                n,
                                                seed,
                                                budget,
                                                candidate_attempts,
                                                first_feasible_ms,
                                            },
                                            candidate,
                                            &selected,
                                        )
                                    })
                                },
                            ));
                        }
                        Err(error) => records.push(QualityBenchmarkRecord {
                            scenario,
                            n,
                            seed,
                            budget_ms: duration_ms(budget),
                            strategy: "orchestrator".to_owned(),
                            selected: false,
                            feasible: false,
                            visited_optional_count: None,
                            visited_optional_priority: None,
                            selected_start_minute: None,
                            finish_minute: None,
                            travel_minutes: None,
                            wait_minutes: None,
                            elapsed_ms: 0.0,
                            candidate_attempts: 0,
                            first_feasible_ms: None,
                            outcome: None,
                            proof_status: None,
                            best_bound: None,
                            relative_gap_ppm: None,
                            thread_count: None,
                            source_version: None,
                            timed_out: matches!(error, troute::solver::SolverError::TimedOut),
                            error: Some(error.to_string()),
                        }),
                    }
                }
            }
        }
    }
    Ok(records)
}

#[derive(Clone, Copy)]
struct QualityRunContext {
    scenario: QualityScenario,
    n: usize,
    seed: u64,
    budget: Duration,
    candidate_attempts: usize,
    first_feasible_ms: Option<f64>,
}

fn quality_candidate_row(
    context: QualityRunContext,
    candidate: SolverCandidate,
    selected_strategy: &str,
) -> QualityBenchmarkRecord {
    let QualityRunContext {
        scenario,
        n,
        seed,
        budget,
        candidate_attempts,
        first_feasible_ms,
    } = context;
    let score = candidate.objective_score;
    QualityBenchmarkRecord {
        scenario,
        n,
        seed,
        budget_ms: duration_ms(budget),
        selected: candidate.strategy == selected_strategy,
        strategy: candidate.strategy,
        feasible: candidate.feasible,
        visited_optional_count: score.map(|value| value.visited_optional_count),
        visited_optional_priority: score.map(|value| value.visited_optional_priority),
        selected_start_minute: score.map(|value| value.start_time_slot),
        finish_minute: score.map(|value| value.finish_time_slot),
        travel_minutes: score.map(|value| value.travel_minutes),
        wait_minutes: score.map(|value| value.wait_minutes),
        elapsed_ms: elapsed_ms(candidate.elapsed),
        candidate_attempts,
        first_feasible_ms,
        outcome: candidate
            .metadata
            .outcome
            .map(|value| format!("{value:?}").to_ascii_lowercase()),
        proof_status: candidate
            .metadata
            .proof_status
            .map(|value| format!("{value:?}").to_ascii_lowercase()),
        best_bound: candidate.metadata.best_bound,
        relative_gap_ppm: candidate.metadata.relative_gap_ppm,
        thread_count: candidate.metadata.thread_count,
        source_version: candidate.metadata.source_version,
        timed_out: candidate.metadata.timed_out,
        error: candidate.metadata.error,
    }
}

fn write_quality_report(
    path: &Path,
    records: &[QualityBenchmarkRecord],
) -> Result<(), Box<dyn Error>> {
    let mut writer = BufWriter::new(File::create(path)?);
    writeln!(writer, "# Solver quality benchmark")?;
    writeln!(writer)?;
    writeln!(
        writer,
        "Selected feasible results at the 5-second wall-clock budget are shown below. Lexicographic objective components are reported separately; no percentage gap is invented when no proven reference exists."
    )?;
    writeln!(writer)?;
    writeln!(
        writer,
        "| scenario | N | seed | strategy | coverage | priority | start | finish | travel | wait |"
    )?;
    writeln!(
        writer,
        "| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |"
    )?;
    for record in records
        .iter()
        .filter(|record| record.budget_ms == 5_000 && record.selected && record.feasible)
    {
        writeln!(
            writer,
            "| {:?} | {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            record.scenario,
            record.n,
            record.seed,
            record.strategy,
            record
                .visited_optional_count
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            record
                .visited_optional_priority
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            record
                .selected_start_minute
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            record
                .finish_minute
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            record
                .travel_minutes
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            record
                .wait_minutes
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        )?;
    }
    writeln!(writer)?;
    writeln!(
        writer,
        "SCIP and LKH-3 rows appear only when explicitly enabled. LKH-3 remains an externally installed, non-redistributed executable. CPU time and peak RSS should be collected by the surrounding OS benchmark harness."
    )?;
    Ok(())
}

fn candidate_row(
    n: usize,
    seed: u64,
    candidate: SolverCandidate,
    selected_strategy: &str,
) -> BenchmarkRecord {
    let metrics = candidate.objective_score.map(MetricsRecord::from);
    BenchmarkRecord {
        n,
        seed,
        strategy: candidate.strategy.clone(),
        feasible: candidate.feasible,
        objective: metrics.map(|value| value.objective),
        latest_start: metrics.map(|value| value.latest_start),
        finish: metrics.map(|value| value.finish),
        travel: metrics.map(|value| value.travel),
        wait: metrics.map(|value| value.wait),
        elapsed_ms: elapsed_ms(candidate.elapsed),
        generated_states: candidate.metadata.state_count,
        frontier_states: candidate.metadata.frontier_state_count,
        cluster_count: candidate.metadata.cluster_count,
        iteration_count: candidate.metadata.iteration_count,
        accepted_moves: candidate.metadata.accepted_moves,
        improved_moves: candidate.metadata.improved_moves,
        initial_score: None,
        objective_gap: None,
        latest_start_gap: None,
        finish_gap: None,
        travel_gap: None,
        wait_gap: None,
        selected: candidate.strategy == selected_strategy,
        timed_out: candidate.metadata.timed_out,
        error: candidate.metadata.error,
    }
}

fn cross_check_matching(seeds: &[u64]) -> Result<(), Box<dyn Error>> {
    for &seed in seeds {
        for size in [4_usize, 8, 12, 16] {
            let mut rng = StdRng::seed_from_u64(seed ^ size as u64);
            let mut distances = vec![vec![0_u64; size]; size];
            let mut left = 0;
            while left < size {
                let mut right = left + 1;
                while right < size {
                    let value = rng.gen_range(1..=10_000);
                    distances[left][right] = value;
                    distances[right][left] = value;
                    right += 1;
                }
                left += 1;
            }
            let vertices: Vec<_> = (0..size).collect();
            let bit_dp = BitDpPerfectMatching::default()
                .minimum_weight_perfect_matching(&vertices, &distances)?;
            let blossom =
                BlossomPerfectMatching.minimum_weight_perfect_matching(&vertices, &distances)?;
            let bit_cost: u64 = bit_dp.iter().map(|edge| edge.distance).sum();
            let blossom_cost: u64 = blossom.iter().map(|edge| edge.distance).sum();
            if bit_cost != blossom_cost {
                return Err(format!(
                    "MWPM mismatch: n={size}, seed={seed}, bit_dp={bit_cost}, blossom={blossom_cost}"
                )
                .into());
            }
        }
    }
    Ok(())
}

fn brute_force(case: &GeneratedCase) -> Option<(SolverSolution, SolutionMetrics)> {
    let n = case.problem.locations().len();
    let mut middle: Vec<_> = (1..n - 1).collect();
    let mut best = None;
    loop {
        let solution = SolverSolution {
            visit_order: std::iter::once(0)
                .chain(middle.iter().copied())
                .chain(std::iter::once(n - 1))
                .collect(),
        };
        if let Some(metrics) = independent_evaluate(case, &solution) {
            if best
                .as_ref()
                .map(|(_, current)| compare_metrics(&metrics, current).is_lt())
                .unwrap_or(true)
            {
                best = Some((solution, metrics));
            }
        }
        if !next_permutation(&mut middle) {
            break;
        }
    }
    best
}

fn independent_evaluate(
    case: &GeneratedCase,
    solution: &SolverSolution,
) -> Option<SolutionMetrics> {
    let earliest = u32::from(case.problem.start_time().minutes()).div_ceil(TIME_SLOT_MINUTES);
    for start in (earliest..24 * 60 / TIME_SLOT_MINUTES).rev() {
        let mut slot = start;
        let mut travel_total = 0_u32;
        let mut wait_total = 0_u32;
        let mut feasible = true;
        for edge in solution.visit_order.windows(2) {
            let travel = case.matrix.travel_minutes(edge[0], edge[1])?;
            travel_total = travel_total.checked_add(travel)?;
            let arrival = slot.checked_add(travel.div_ceil(TIME_SLOT_MINUTES))?;
            let location = &case.problem.locations()[edge[1]];
            let open =
                u32::from(location.time_window().open().minutes()).div_ceil(TIME_SLOT_MINUTES);
            let service_start = arrival.max(open);
            wait_total = wait_total.checked_add((service_start - arrival) * TIME_SLOT_MINUTES)?;
            slot =
                service_start.checked_add(location.stay_minutes().div_ceil(TIME_SLOT_MINUTES))?;
            let close = u32::from(location.time_window().close().minutes()) / TIME_SLOT_MINUTES;
            if slot > close || slot >= 24 * 60 / TIME_SLOT_MINUTES {
                feasible = false;
                break;
            }
        }
        if feasible {
            return Some(SolutionMetrics {
                start_policy: case.problem.start_policy(),
                visited_optional_count: 0,
                visited_optional_priority: 0,
                start_time_slot: start as u16,
                finish_time_slot: slot as u16,
                travel_minutes: travel_total,
                wait_minutes: wait_total,
                score: u64::from(travel_total) + u64::from(wait_total),
            });
        }
    }
    None
}

fn compare_metrics(left: &SolutionMetrics, right: &SolutionMetrics) -> Ordering {
    right
        .start_time_slot
        .cmp(&left.start_time_slot)
        .then_with(|| left.finish_time_slot.cmp(&right.finish_time_slot))
        .then_with(|| left.travel_minutes.cmp(&right.travel_minutes))
        .then_with(|| left.wait_minutes.cmp(&right.wait_minutes))
        .then_with(|| left.score.cmp(&right.score))
}

fn next_permutation(values: &mut [usize]) -> bool {
    let Some(pivot) = (0..values.len().saturating_sub(1))
        .rev()
        .find(|&index| values[index] < values[index + 1])
    else {
        return false;
    };
    let successor = (pivot + 1..values.len())
        .rev()
        .find(|&index| values[pivot] < values[index])
        .unwrap();
    values.swap(pivot, successor);
    values[pivot + 1..].reverse();
    true
}

fn hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}
