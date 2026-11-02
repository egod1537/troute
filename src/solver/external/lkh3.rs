use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use tempfile::Builder;

use crate::{
    domain::VisitPolicy,
    solver::{
        OrchestratedStrategy, SolverCandidateMetadata, SolverError, SolverInput, SolverSolution,
        StrategyExecution,
    },
};

const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_DIAGNOSTIC_BYTES: u64 = 64 * 1024;
const MAX_LKH3_LOCATIONS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lkh3Config {
    pub executable: PathBuf,
    pub time_limit: Duration,
    pub seed: u64,
}

#[derive(Debug, Clone)]
pub struct Lkh3Strategy {
    config: Lkh3Config,
}

impl Lkh3Strategy {
    pub fn new(mut config: Lkh3Config) -> Result<Self, SolverError> {
        if config.time_limit.is_zero() {
            return Err(SolverError::InvalidConfiguration(
                "LKH-3 time limit must be positive".to_owned(),
            ));
        }
        config.executable = config.executable.canonicalize().map_err(|error| {
            SolverError::InvalidConfiguration(format!(
                "cannot resolve LKH3_EXECUTABLE {}: {error}",
                config.executable.display()
            ))
        })?;
        if !config.executable.is_file() {
            return Err(SolverError::InvalidConfiguration(format!(
                "LKH3_EXECUTABLE is not a file: {}",
                config.executable.display()
            )));
        }
        Ok(Self { config })
    }
}

impl OrchestratedStrategy for Lkh3Strategy {
    fn name(&self) -> &str {
        "lkh3"
    }

    fn is_applicable(&self, location_count: usize) -> bool {
        (2..=MAX_LKH3_LOCATIONS).contains(&location_count)
    }

    fn is_applicable_to(&self, problem: &crate::domain::OptimizationProblem) -> bool {
        problem.visit_policy() == VisitPolicy::AllRequired
            && self.is_applicable(problem.locations().len())
    }

    fn execute(&self, input: SolverInput<'_>) -> Result<StrategyExecution, SolverError> {
        execute_lkh3(input, &self.config)
    }
}

fn execute_lkh3(
    input: SolverInput<'_>,
    config: &Lkh3Config,
) -> Result<StrategyExecution, SolverError> {
    if input.cancellation.is_cancelled() {
        return Err(SolverError::Cancelled);
    }
    let transformed = OpenPathTransformation::new(input)?;
    let temp_dir = Builder::new()
        .prefix("troute-lkh3-")
        .tempdir()
        .map_err(|error| SolverError::Failed(format!("cannot create LKH-3 temp dir: {error}")))?;
    let problem_path = temp_dir.path().join("problem.atsp");
    let parameter_path = temp_dir.path().join("params.par");
    let tour_path = temp_dir.path().join("result.tour");
    let stdout_path = temp_dir.path().join("stdout.log");
    let stderr_path = temp_dir.path().join("stderr.log");
    write_text(&problem_path, &transformed.problem_text())?;
    write_text(
        &parameter_path,
        &parameter_text(config.time_limit, config.seed),
    )?;

    let stdout = File::create(&stdout_path)
        .map_err(|error| SolverError::Failed(format!("cannot create LKH stdout file: {error}")))?;
    let stderr = File::create(&stderr_path)
        .map_err(|error| SolverError::Failed(format!("cannot create LKH stderr file: {error}")))?;
    let mut child = Command::new(&config.executable)
        .arg("params.par")
        .current_dir(temp_dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| {
            SolverError::Failed(format!(
                "cannot start LKH-3 executable {}: {error}",
                config.executable.display()
            ))
        })?;
    let child_process_id = child.id();
    let deadline = Instant::now() + config.time_limit;
    let status = loop {
        if input.cancellation.is_cancelled() {
            terminate_process_tree(&mut child);
            return Err(SolverError::Cancelled);
        }
        if Instant::now() >= deadline {
            terminate_process_tree(&mut child);
            return Err(SolverError::TimedOut);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(PROCESS_POLL_INTERVAL),
            Err(error) => {
                terminate_process_tree(&mut child);
                return Err(SolverError::Failed(format!(
                    "cannot poll LKH-3 process: {error}"
                )));
            }
        }
    };
    let exit_code = status.code();
    ensure_success(status, &stdout_path, &stderr_path)?;
    let output = std::fs::read_to_string(&tour_path).map_err(|error| {
        SolverError::Failed(format!("LKH-3 did not produce result.tour: {error}"))
    })?;
    let cycle = parse_tour(&output, transformed.dimension)?;
    let route = transformed.decode(cycle)?;
    Ok(StrategyExecution {
        solution: SolverSolution {
            visit_order: route.clone(),
        },
        metadata: SolverCandidateMetadata {
            source_version: Some("LKH-3 user-supplied executable".to_owned()),
            native_status: Some("EXIT_SUCCESS".to_owned()),
            child_process_id: Some(child_process_id),
            exit_code,
            thread_count: Some(1),
            seed: Some(config.seed),
            final_route: Some(
                route
                    .iter()
                    .map(|&index| input.problem.locations()[index].id().to_owned())
                    .collect(),
            ),
            ..SolverCandidateMetadata::default()
        },
    })
}

struct OpenPathTransformation {
    dimension: usize,
    start: usize,
    end: usize,
    dummy: usize,
    weights: Vec<Vec<u64>>,
}

impl OpenPathTransformation {
    fn new(input: SolverInput<'_>) -> Result<Self, SolverError> {
        let count = input.problem.locations().len();
        let dimension = count + 1;
        let start = input.problem.start_location_index();
        let end = input.problem.end_location_index();
        let dummy = count;
        let max_arc = (0..count)
            .flat_map(|from| (0..count).filter_map(move |to| input.matrix.travel_minutes(from, to)))
            .map(u64::from)
            .max()
            .unwrap_or(1)
            .max(1);
        let forbidden = max_arc
            .checked_add(1)
            .and_then(|value| value.checked_mul(dimension as u64 + 1))
            .ok_or_else(|| {
                SolverError::Unsupported("LKH-3 edge weights overflow u64".to_owned())
            })?;
        if forbidden > i32::MAX as u64 {
            return Err(SolverError::Unsupported(
                "LKH-3 transformed edge weights exceed 32-bit precision".to_owned(),
            ));
        }
        let mut weights = vec![vec![forbidden; dimension]; dimension];
        for (from, row) in weights.iter_mut().enumerate().take(count) {
            for (to, weight) in row.iter_mut().enumerate().take(count) {
                if from == to || to == start || from == end {
                    continue;
                }
                if let Some(travel) = input.matrix.travel_minutes(from, to) {
                    *weight = u64::from(travel) + 1;
                }
            }
        }
        weights[end][dummy] = 1;
        weights[dummy][start] = 1;
        Ok(Self {
            dimension,
            start,
            end,
            dummy,
            weights,
        })
    }

    fn problem_text(&self) -> String {
        let mut text = format!(
            "NAME: TROUTE_OPEN_PATH\nTYPE: ATSP\nDIMENSION: {}\nEDGE_WEIGHT_TYPE: EXPLICIT\nEDGE_WEIGHT_FORMAT: FULL_MATRIX\nEDGE_WEIGHT_SECTION\n",
            self.dimension
        );
        for row in &self.weights {
            text.push_str(&row.iter().map(u64::to_string).collect::<Vec<_>>().join(" "));
            text.push('\n');
        }
        text.push_str("EOF\n");
        text
    }

    fn decode(&self, cycle: Vec<usize>) -> Result<Vec<usize>, SolverError> {
        let start_position = cycle
            .iter()
            .position(|&node| node == self.start)
            .ok_or_else(|| {
                SolverError::Failed("LKH-3 tour omitted the fixed start node".to_owned())
            })?;
        let mut rotated = cycle[start_position..]
            .iter()
            .chain(cycle[..start_position].iter())
            .copied()
            .collect::<Vec<_>>();
        if rotated.last() != Some(&self.dummy)
            || rotated.get(rotated.len().saturating_sub(2)) != Some(&self.end)
        {
            return Err(SolverError::Failed(
                "LKH-3 tour violated the fixed open-path end transformation".to_owned(),
            ));
        }
        rotated.pop();
        Ok(rotated)
    }
}

fn parameter_text(time_limit: Duration, seed: u64) -> String {
    let seconds = time_limit.as_secs_f64().ceil().max(1.0) as u64;
    format!(
        "PROBLEM_FILE = problem.atsp\nOUTPUT_TOUR_FILE = result.tour\nRUNS = 1\nSEED = {seed}\nTIME_LIMIT = {seconds}\nTRACE_LEVEL = 0\n"
    )
}

fn parse_tour(text: &str, dimension: usize) -> Result<Vec<usize>, SolverError> {
    let mut in_tour = false;
    let mut nodes = Vec::with_capacity(dimension);
    for line in text.lines().map(str::trim) {
        if line.eq_ignore_ascii_case("TOUR_SECTION") {
            in_tour = true;
            continue;
        }
        if !in_tour {
            continue;
        }
        if line == "-1" || line.eq_ignore_ascii_case("EOF") {
            break;
        }
        let node = line
            .parse::<usize>()
            .map_err(|_| SolverError::Failed(format!("invalid LKH-3 tour node: {line:?}")))?;
        if !(1..=dimension).contains(&node) {
            return Err(SolverError::Failed(format!(
                "LKH-3 tour node {node} is outside 1..={dimension}"
            )));
        }
        nodes.push(node - 1);
    }
    let mut unique = nodes.clone();
    unique.sort_unstable();
    unique.dedup();
    if nodes.len() != dimension || unique.len() != dimension {
        return Err(SolverError::Failed(format!(
            "LKH-3 tour must contain every transformed node exactly once (expected {dimension}, got {})",
            nodes.len()
        )));
    }
    Ok(nodes)
}

fn write_text(path: &Path, text: &str) -> Result<(), SolverError> {
    let mut file = File::create(path).map_err(|error| {
        SolverError::Failed(format!("cannot create {}: {error}", path.display()))
    })?;
    file.write_all(text.as_bytes())
        .map_err(|error| SolverError::Failed(format!("cannot write {}: {error}", path.display())))
}

fn ensure_success(
    status: ExitStatus,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<(), SolverError> {
    if status.success() {
        return Ok(());
    }
    let stdout = read_capped(stdout_path);
    let stderr = read_capped(stderr_path);
    Err(SolverError::Failed(format!(
        "LKH-3 exited with {status}; stdout={stdout:?}; stderr={stderr:?}"
    )))
}

fn read_capped(path: &Path) -> String {
    let Ok(file) = File::open(path) else {
        return String::new();
    };
    let mut output = String::new();
    let _ = file.take(MAX_DIAGNOSTIC_BYTES).read_to_string(&mut output);
    output
}

fn terminate_process_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests;
