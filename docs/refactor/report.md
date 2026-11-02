# Structural refactor report

## 1. Baseline

Work started from `671005889802e79f1c7b4d2c07ecf36fa39e6a3b` on branch `impl`, with pre-existing local changes and a user-owned replacement of the `spec/` set. The implementation preserved those files and did not reset or rewrite history. Validation was performed on Windows with Rust/Cargo 1.99.1 using `stable-x86_64-pc-windows-gnu`; the default Windows linker path selected an incompatible MSYS `link.exe`.

See [baseline.md](baseline.md) for the initial file sizes, dirty-state boundary, and platform notes.

## 2. Structure change summary

| Previous responsibility | New owner | Simplified caller |
| --- | --- | --- |
| environment parsing/defaulting throughout `main` and router startup | `AppConfig`, `ServerConfig`, `RoutingConfig`, typed `EnvReader` | `main` now loads config and calls `server::run` |
| provider/solver/storage/router/bind/shutdown in `main` | staged functions in `server.rs` | top-level entry remains 20 lines and preserves `ExitCode` behavior |
| duplicate orchestrator worker loops | `BoundedStrategyExecution` plus explicit `StrategyTimeoutPolicy` | registered and anytime baseline phases share mechanics without merging budgets |
| scattered candidate/best/update state | `CandidateAccumulator` | exact, baseline, and SA phases use one invariant owner |
| SA current/best/temperature/counters | `AnnealingSearchState` | `solve_detailed` exposes setup, loop, verification, result |
| optimization mega-flow | `PreparedOptimization`, `ResolvedRouting`, `FinalizationInput` and named stages | `optimize_with_cancellation` is a pipeline boundary |
| repeated job execution arguments | `JobExecutionContext` | worker spawn, update consumption, and terminal persistence are separate |
| distributed routing atomics/results/error | `MatrixBuildState` | pair workers call common completion/failure transitions |
| HTTP facade mixed with implementations | `http/router.rs`, `http/error.rs`, `http/handlers/{optimization,jobs,events}.rs` | `http.rs` retains shared DTO/state and public facade |
| duplicate storage terminal checks/commits | `prepare_terminal_write`, `commit_terminal_state` | result/error writes retain payload-specific behavior |
| remediation budget/probes/suggestions argument bundles | `RemediationRun` | probe methods share one deadline/count invariant |

## 3. Requirements trace

| Requirement | Status | Code/test evidence |
| --- | --- | --- |
| Remove individual env reads and `read_*` from `main.rs` | Complete | `src/main.rs`, `src/config.rs` |
| Include local `read_f64`/`read_bool` settings | Complete | `EnvReader`; NaN/infinity/boundary and bool tests |
| Introduce `AppConfig` and reuse existing sub-configs | Complete | directly owns `SolverOrchestratorConfig`, `RemediationConfig`, `TcacheRoutingConfig` |
| Single responsibility for defaults/validation/aliases/warnings | Complete | owning defaults plus `from_config_sources`; [compatibility map](compatibility.md) |
| Separate main, server assembly, shutdown | Complete | `src/main.rs`, `src/server.rs`, child-process startup tests |
| Reduce excessive general control-flow nesting | Complete | service, jobs, router, storage, orchestration stages; DP exception documented |
| Common identical repeated logic | Complete | executor, terminal transition, tcache/matching raw parsers |
| Encapsulate shared mutable state and invariants | Complete | candidate, matrix, job, remediation, SA state types |
| Reuse algorithm interfaces; avoid new manager/trait layers | Complete | no new framework trait/Builder/Manager; existing solver/provider traits retained |
| Remove orchestrator queue/worker/candidate repetition | Complete | `BoundedStrategyExecution`, `CandidateAccumulator` |
| Preserve exact/baseline/SA order, comparison, seed behavior | Complete | orchestrator tests and `refactor_regression` |
| Separate Service/HTTP/Job/SSE boundaries | Complete | named service stages and internal HTTP modules |
| Review routing/storage/failure/progress ownership | Complete | matrix and remediation state; terminal transition helper; existing progress coordinator retained |
| `mod.rs`-free layout and public paths | Complete/already satisfied | no `mod.rs`; `cargo check --all-targets`, API tests |
| Regress API/config/disk/cancel/timeout behavior | Complete with one timing-flake note | full test suite plus targeted contracts; see validation |

## 4. Commonality and invariants

`BoundedStrategyExecution` owns work distribution, scoped workers, result collection, and registration-order sorting. Its timeout enum keeps the important distinction between a fixed cap and the remaining global deadline. `CandidateAccumulator` updates the best index only through the shared objective tracker and never discards failed diagnostics.

`MatrixBuildState` owns completion count, cancellation, result slots, first error, and progress serialization. It was intentionally not generalized with solver execution because provider jobs have different cancellation and failure-pair semantics.

`JobExecutionContext` keeps request, ID, cancellation token, and pacing together while `JobRunner` continues to own the semaphore, registry, executor, and store. Permit/registry lifetime and terminal-write-before-event ordering are unchanged.

`AnnealingSearchState` owns actual mutable search state, not static helpers. Neighbor selection and `rng.gen::<f64>()` remain in their previous order, best feasible candidates are recorded before acceptance, and fixed-seed regression output is unchanged.

Exact DP mask/frontier nesting remains intentional. No matrix clone, request clone in a hot loop, or new dynamic dispatch was introduced. Handler extraction moved ownership rather than changing DTOs.

## 5. Compatibility

The complete environment, provider, HTTP/SSE, disk, and Rust API inventory is in [compatibility.md](compatibility.md). Important verified points:

- stderr still starts with `troute server error: ` and failures return a nonzero exit code;
- tcache new aliases override legacy aliases even when the new value is invalid;
- warning-and-default HTTP limits remain non-fatal and warnings are emitted once at startup;
- provider selection priority and deprecated JP behavior are unchanged;
- supplied matrices still bypass routing calls and routing-pair limits;
- SSE remains persisted-state authoritative and terminal events follow applied writes;
- file names, JSON/JSONL schemas, recovery meaning, public module paths, edition, MSRV, features, and dependencies are unchanged.

## 6. Validation

Commands use an isolated `%TEMP%/troute-spec-validation` target directory.

| Command | Result |
| --- | --- |
| `cargo +stable-x86_64-pc-windows-gnu fmt --all -- --check` | Pass |
| `cargo +stable-x86_64-pc-windows-gnu check --locked --all-targets` | Pass |
| `cargo +stable-x86_64-pc-windows-gnu clippy --locked --all-targets -- -D warnings` | Pass |
| targeted SA/config/orchestrator/service/jobs/routing/storage tests | Pass |
| `cargo ... test --test refactor_regression` | 3 passed |
| `cargo ... test --test v0_contract` | 7 passed |
| `cargo ... test --test server_startup` | 2 passed |
| `cargo ... test --test http_api` | 37 passed, one timing assertion failed once; isolated rerun passed |
| `cargo ... test --locked` | One failure: `pending_queue_time_counts_toward_debug_minimum`; all preceding unit/config/algorithm tests passed, and the failed test passes in isolation |
| `cargo ... test --locked -- --test-threads=1` | Pass: 179 library, 9 config, 38 HTTP, and all integration/doc tests; one live tcache test remained intentionally ignored |
| `cargo ... test --locked --all-targets --no-run` | Pass |
| `cargo ... test --locked --test algorithm_validation` | Pass |
| `cargo ... test --locked --bench clustered_solver` | Pass |
| `python tests/test_compose_ports.py` | 2 passed |
| `python -m unittest discover -s tests -p "test_*.py"` | Not runnable as a suite on this Windows host: 35 deploy-test errors because `bash` is unavailable; the two platform-independent compose tests passed separately |
| `bash scripts/check_refactor_regression.sh` | Not invoked because `bash` is unavailable; every Rust command listed by the script was run directly with the GNU toolchain, with the parallel timing result reported above |

No live/ignored network test was run. The deployment tests failed before invoking their mocked scripts because their required `bash` executable is not installed on this Windows host; this is an environment limitation, not a passing result.

## 7. Performance

The HEAD baseline and current tree were built in separate temporary worktrees with GNU release mode. `benches/clustered_solver.rs` used the same synthetic directed matrices, seed 42, default concurrency, and five processes per tree. Values are median wall-clock microseconds; five samples are insufficient for a meaningful p95.

| Case | HEAD median | Refactor median | Change |
| --- | ---: | ---: | ---: |
| Exact, N=12 | 1,707 us | 1,454 us | -14.8% |
| Clustered, N=12 | 41 us | 50 us | +22.0% (9 us absolute; samples ranged 38–98 us after refactor) |
| SA, N=12 | 2,036 us | 2,112 us | +3.7% |
| Clustered, N=40 | 3,698 us | 3,774 us | +2.1% |
| SA, N=40 | 4,111 us | 4,140 us | +0.7% |
| N=40 orchestrator total | 69.317 s | 69.258 s | -0.1% |

All runs preserved exact score/state/frontier counts, clustered scores, SA score/iterations/accepted/improved counts, and selected strategy. The N=12 clustered percentage crosses the provisional 10% review threshold, but the measured difference is 9 microseconds and its within-version spread is larger than the difference; no clustered algorithm code changed. Larger cases and end-to-end orchestration show no corresponding regression. This is treated as timer/scheduling noise, not proof of an improvement or regression. Peak RSS, allocation counts, Job/SSE throughput, and statistically valid p95 remain unmeasured.

## 8. Remaining risks and deliberately untouched areas

- The queue pacing HTTP test has a known wall-clock scheduling edge; it was not weakened or given a larger timeout.
- Cooperative cancellation still cannot hard-preempt arbitrary blocking solver code; scoped workers may wait to join as before.
- Storage remains a large facade because public schemas, traits, fixtures, and I/O are tightly coupled by compatibility. Only shared terminal transition policy was extracted.
- Peak memory/allocation and production provider latency were not measured. No live credentialed tests were authorized.
- No new feature, algorithm, retry policy, data migration, `.env` loading, or CLI precedence was introduced.

## 9. Change scale

The refactor adds configuration/server/error modules and one child-process test, while moving handler/SSE implementations out of `http.rs`. `main.rs` decreased from roughly 400 lines to 20 and `http.rs` from roughly 1,507 to 390. State-owning refactors add lines in service, orchestration, routing, jobs, storage, and SA; this is justified by explicit invariants rather than a claim based on total LOC. Large exact DP loops and declaration/test-heavy facades remain documented exceptions.
