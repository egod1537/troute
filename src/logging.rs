use std::{
    fmt::{self, Display},
    net::SocketAddr,
    num::NonZeroUsize,
    path::Path,
    sync::OnceLock,
    time::Duration,
};

use crate::{
    routing::RouteProviderSelectionDiagnostics, solver::SolverOrchestratorConfig, RemediationConfig,
};

static APP_LOGGER: OnceLock<AppLogger> = OnceLock::new();

/// Process-wide application logger.
///
/// The logger is immutable after initialization. Event call sites decide when
/// something happened; this type owns how operational events are rendered.
#[derive(Debug, Default)]
pub struct AppLogger {
    _private: (),
}

/// Initializes the process-wide logger before application startup.
pub fn init() {
    APP_LOGGER.get_or_init(AppLogger::default);
}

/// Returns the shared logger, lazily initializing it for library-only callers.
pub fn logger() -> &'static AppLogger {
    APP_LOGGER.get_or_init(AppLogger::default)
}

/// Data rendered when all production server components have been initialized.
pub struct ServerStartupLog<'a> {
    pub bind_addr: SocketAddr,
    pub data_dir: &'a Path,
    pub recovered_jobs: usize,
    pub solver: &'a SolverOrchestratorConfig,
    pub provider: &'a RouteProviderSelectionDiagnostics,
    pub pair_concurrency: NonZeroUsize,
    pub matrix_total_timeout: Duration,
    pub remediation: RemediationConfig,
}

impl AppLogger {
    fn info(&self, message: fmt::Arguments<'_>) {
        println!("{message}");
    }

    fn warning(&self, message: fmt::Arguments<'_>) {
        eprintln!("{message}");
    }

    pub fn startup_warning(&self, warning: &str) {
        self.warning(format_args!("warning: {warning}"));
    }

    pub fn server_error<E>(&self, error: &E)
    where
        E: Display + ?Sized,
    {
        self.warning(format_args!("troute server error: {error}"));
    }

    pub fn signal_listener_failed<E>(&self, signal: &str, error: &E)
    where
        E: Display + ?Sized,
    {
        self.warning(format_args!(
            "troute server error: cannot listen for {signal}: {error}"
        ));
    }

    pub fn server_starting(&self, startup: ServerStartupLog<'_>) {
        let ServerStartupLog {
            bind_addr,
            data_dir,
            recovered_jobs,
            solver,
            provider,
            pair_concurrency,
            matrix_total_timeout,
            remediation,
        } = startup;

        self.info(format_args!(
            "troute server starting; bind address: {bind_addr}"
        ));
        self.info(format_args!(
            "Local job data directory: {}",
            data_dir.display()
        ));
        self.info(format_args!("Recovered interrupted jobs: {recovered_jobs}"));
        self.info(format_args!(
            "Exact solver location limit: {}",
            solver.exact_limit
        ));
        self.info(format_args!(
            "Maximum exact cluster size: {}",
            solver.max_cluster_size
        ));
        self.info(format_args!(
            "Solver strategy concurrency: {}",
            solver.max_concurrency
        ));
        self.info(format_args!("tcache pair concurrency: {pair_concurrency}"));
        self.info(format_args!(
            "tcache matrix total timeout: {} ms",
            matrix_total_timeout.as_millis()
        ));
        self.info(format_args!(
            "Route provider mode: {} (request override enabled: {})",
            provider.route_provider_mode, provider.override_enabled
        ));
        self.info(format_args!(
            "Per-strategy timeout: {} ms",
            solver.strategy_timeout.as_millis()
        ));
        self.info(format_args!(
            "Solver total budget: {} ms",
            solver.total_budget.as_millis()
        ));
        self.info(format_args!("SA chunk: {} ms", solver.sa_chunk.as_millis()));
        self.info(format_args!(
            "Portfolio policy: {}; telemetry: {:?}; experiment override: {}",
            solver.portfolio.policy, solver.telemetry_mode, solver.experiment_override_enabled
        ));
        self.info(format_args!(
            "Deadline safety margin: {} ms",
            solver.deadline_safety_margin.as_millis()
        ));
        self.info(format_args!(
            "SCIP strategy: {}; cap: {} ms; threads: {}; memory: {} MiB",
            if solver.enable_mip_scip {
                "enabled"
            } else {
                "disabled"
            },
            solver.mip_scip_time_limit.as_millis(),
            solver.mip_scip_threads,
            solver.mip_scip_memory_mb
        ));
        self.info(format_args!(
            "LKH-3 strategy: {}; cap: {} ms; compare-all: {}; incumbent exchange: {}",
            if !solver.enable_lkh3 {
                "disabled"
            } else if solver.lkh3_executable.as_deref().is_some_and(Path::is_file) {
                "available"
            } else {
                "unavailable (LKH3_EXECUTABLE missing)"
            },
            solver.lkh3_time_limit.as_millis(),
            solver.compare_all,
            solver.incumbent_exchange
        ));
        self.info(format_args!(
            "Failure remediation budget: {} ms; max remove probes: {}; max suggestions: {}",
            remediation.budget.as_millis(),
            remediation.max_remove_probes,
            remediation.max_suggestions
        ));
    }

    pub fn server_listening(&self, bind_addr: SocketAddr) {
        self.info(format_args!(
            "troute server listening on http://{bind_addr}"
        ));
    }

    pub fn server_shutting_down(&self) {
        self.info(format_args!("troute server shutting down"));
    }

    pub fn server_stopped(&self) {
        self.info(format_args!("troute server stopped"));
    }

    pub fn job_event_sequence_persistence_failed<E>(&self, job_id: &str, sequence: u64, error: &E)
    where
        E: Display + ?Sized,
    {
        self.warning(format_args!(
            "job event sequence persistence failed; job_id={job_id}; sequence={sequence}; error={error}"
        ));
    }

    pub fn job_event_publish_skipped_missing(&self, job_id: &str) {
        self.warning(format_args!(
            "job event publish skipped; job_id={job_id}; persisted job disappeared"
        ));
    }

    pub fn job_event_publish_skipped<E>(&self, job_id: &str, error: &E)
    where
        E: Display + ?Sized,
    {
        self.warning(format_args!(
            "job event publish skipped; job_id={job_id}; error={error}"
        ));
    }

    pub fn debug_job_pacing_enabled(&self, job_id: &str, minimum_duration_ms: u64) {
        self.warning(format_args!(
            "debug job pacing enabled; job_id={job_id}; min_job_duration_ms={minimum_duration_ms}"
        ));
    }

    pub fn job_operation_failed<E>(&self, operation: &str, job_id: &str, error: &E)
    where
        E: Display + ?Sized,
    {
        self.warning(format_args!(
            "{operation} failed; job_id={job_id}; error={error}"
        ));
    }

    pub fn job_operation_panicked(&self, operation: &str, job_id: &str) {
        self.warning(format_args!("{operation} failed; job_id={job_id}"));
    }

    pub fn malformed_timeline_line<E>(&self, job_id: &str, error: &E)
    where
        E: Display + ?Sized,
    {
        self.warning(format_args!(
            "ignoring malformed timeline line; job_id={job_id}; error={error}"
        ));
    }

    pub fn http_request_failed(&self, code: &str, status: u16, reason: &str) {
        self.warning(format_args!(
            "HTTP request failed; code={code}; status={status}; reason={reason}"
        ));
    }

    pub fn invalid_environment_value<D>(&self, name: &str, value: &str, default: D)
    where
        D: Display,
    {
        self.warning(format_args!(
            "invalid {name}={value:?}; using default {default}"
        ));
    }

    pub fn environment_read_failed<E, D>(&self, name: &str, error: &E, default: D)
    where
        E: Display + ?Sized,
        D: Display,
    {
        self.warning(format_args!(
            "cannot read {name}: {error}; using default {default}"
        ));
    }

    pub fn tcache_route_job_timed_out(&self, from: &str, to: &str, job_id: &str) {
        self.warning(format_args!(
            "tcache route job timed out; pair={from} -> {to}; jobId={job_id}"
        ));
    }
}
