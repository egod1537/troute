# Portfolio scheduler architecture

The dependency direction is:

```text
HTTP / Job
  -> RouteOptimizationService
    -> SolverOrchestrator (compatibility facade)
      -> portfolio policy + registry
        -> ResourceManager
          -> OrchestratedStrategy adapter
            -> Exact / MIP / external / heuristic solver
      -> common ObjectiveEvaluator + BestSolutionTracker
      -> TelemetryCollector
```

The scheduler has no dependency on Axum, SSE, JSON request handlers, routing
providers, or Job storage. `service.rs` translates solver telemetry to the
application reporter; `jobs.rs` persists those events through `JobStore`.
State is owned by one solve call. Only the resource manager is shared by calls,
and it contains capacity counters rather than candidate or policy history.

## Legacy CURRENT call graph

`RouteSolver::solve_with_portfolio_options` enters
`SolverOrchestrator::solve_detailed_with_progress`. With `CURRENT` it preserves
the existing sequence:

1. Run Exact Bit DP first when `N <= exact_limit`.
2. Return immediately after a validated exact solution unless `compare_all` is
   enabled.
3. Queue applicable baseline strategies and run at most the configured logical
   CPU slots.
4. Run simulated annealing in bounded chunks. The legacy four initializer
   order and periodic global-best warm start are preserved.
5. Validate every returned route with the shared `ObjectiveEvaluator`.
6. Rank validated candidates with the existing lexicographic comparator and
   assemble legacy diagnostics and response fields.

The `algorithm_validation` legacy result generator has historically included
setup/report work outside its requested solver budget. Policy comparisons use
`portfolio_benchmark`, whose manifest and each run record both store the actual
configured budget; reporting time is not labelled solver time.

## Responsibility boundaries

| Boundary | Responsibility |
| --- | --- |
| `context.rs` | Per-run identity, monotonic deadlines, cancellation, deterministic seed, warm start, and owned RAII lease |
| `descriptor.rs` | Stable Solver IDs, capability/availability metadata, Registry enumeration and applicability |
| `policy.rs` | Eight policy IDs, decisions, arm observations, seeded RR/weighted/racing/UCB1/Thompson selection, input-only instance rules |
| `resources.rs` | RAII logical slots, native-thread reservations, external-process and memory hints, finite cancellation-aware waits |
| `objective.rs` | Objective vector, route fingerprint, atomic validated-best update and policy reward |
| `telemetry.rs` | Versioned event schema, central sequence assignment, bounded collection, run and scheduler summaries |
| `orchestrator/runner.rs` | Compatibility facade, common execution/validation path, policy-to-strategy adaptation |
| `storage.rs` | Append-only Job telemetry JSONL and cursor reads; never decides solver quality |

## Objective and reward

Candidate selection always uses the existing lexicographic order:

1. optional visit count, descending;
2. optional priority sum, descending;
3. requested start policy (`FIXED`, earliest ascending, latest descending);
4. finish time, ascending;
5. travel, ascending;
6. wait, ascending.

Bandit reward is separate and bounded: a validated global-best improvement is
1, otherwise 0. It cannot reverse candidate ordering. Identical candidates do
not receive another improvement reward.

## Capability matrix

| Solver | Optional | Sparse directed | Direct time windows | Proof | Restartable | Pause/resume | External |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Exact Bit DP | yes | yes | yes | yes | no | no | no |
| SCIP MIP | yes | yes | yes | multi-phase | no | no | native library |
| LKH-3 | no | no | **no** (ATSP candidate only) | no | yes, new process/run | no | yes |
| Clustered | yes | yes | yes | no | no | no | no |
| Greedy/MST/Christofides | no | descriptor-specific | common validation | no | no | no | no |
| Simulated annealing | yes | yes | common validation | no | yes, new run | no | no |

Missing Cargo features, disabled settings, unsupported input shapes, and a
missing LKH executable are represented separately from failed executions.

## Policy behavior and non-preemptible work

- `CURRENT`: exact-first legacy flow and global-best SA warm starts.
- `PARALLEL_ALL`: queues every applicable baseline immediately; physical start
  remains bounded by resource leases.
- `ROUND_ROBIN`: rotates restartable SA arms. It does not claim OS-level
  preemption of Exact, Clustered, SCIP, or an active child process.
- `WEIGHTED_FAIR`: chooses the lowest reserved slot-time divided by configured
  positive weight; unused capacity remains work-conserving.
- `RACING`: gives each restartable arm a warm-up, then deterministically removes
  the lower half at doubling observation stages.
- `UCB1`: explores every arm once, then uses duration-aligned
  `mean + c * sqrt(ln N / n)`.
- `THOMPSON_SAMPLING`: samples a Beta posterior for the Bernoulli global-best
  improvement event with the per-Job seed.
- `INSTANCE_AWARE`: records input-only features and applies deterministic rules;
  it never uses a future result as an initial feature.

SCIP and other long-running solvers keep a protected, non-preemptible run. A
cancelled and restarted LKH/SA invocation receives a new Run ID.

## Cancellation and terminal states

```text
QUEUED --lease--> RUNNING --> FINISHED
   |                 |  |----> TIMED_OUT
   |                 |-------> FAILED
   +--cancel/deadline---------> CANCELLED / TIMED_OUT
```

Job cancellation is checked while waiting for a lease and inside every solver
adapter. Leases release on success, error, panic, timeout, and cancellation.
LKH terminates its child; SCIP receives its configured interrupt/deadline.
Rust cannot safely force-kill an arbitrary in-process native worker, so a
non-cooperative native call may delay thread join; telemetry and documentation
do not call this pause/resume.

Configured native threads, nominal reserved slot-ms, measured process CPU-ms,
and RSS are different fields. Unsupported measurements remain null.
