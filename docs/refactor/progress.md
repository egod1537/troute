# Refactor progress

## Phase status

| Phase | Status | Evidence |
| --- | --- | --- |
| Baseline and compatibility inventory | Complete | `baseline.md`, `compatibility.md` |
| `AppConfig` and testable environment source | Complete | `src/config.rs`, `src/config/env.rs`, map-based unit tests |
| Main/server/shutdown separation | Complete | `src/main.rs`, `src/server.rs`, `tests/server_startup.rs` |
| Orchestrator execution and candidate state | Complete | `BoundedStrategyExecution`, `StrategyTimeoutPolicy`, `CandidateAccumulator` |
| Solver algorithm boundary review | Complete | SA `AnnealingSearchState`; exact DP nesting retained as an algorithmic exception |
| Routing shared state | Complete | `MatrixBuildState` in `routing/matrix_builder.rs` |
| Service pipeline | Complete | prepared, routing, matrix, solve, and finalization stages in `service.rs` |
| Job lifecycle | Complete | `JobExecutionContext` and separated worker/update/persistence stages |
| HTTP/error/handler/SSE ownership | Complete | `http/error.rs`, `http/handlers/*`, `http/router.rs` |
| Storage terminal transition commonality | Complete | shared terminal precondition and commit helpers under the existing lock |
| Failure remediation state | Complete | `RemediationRun` owns deadline, token, probe count, and suggestions |
| `mod.rs` layout | Already satisfied | no directly managed `mod.rs` found |
| Regression and performance validation | Complete | commands and measurements in `report.md` |

## Commonality decisions

| Call sites | Shared mechanism | Deliberately separate policy |
| --- | --- | --- |
| registered strategies, anytime baselines | bounded queue/worker/result collection and registration-order sorting | fixed per-strategy timeout vs remaining global budget |
| exact, baseline, SA candidates | candidate storage, best-index invariant, best update count | phase order, seed derivation, restart/warm-start policy |
| routing pair workers | matrix result slots, first failure, completed count, cancellation | not combined with the CPU solver executor because I/O cancellation and diagnostics differ |
| result and error terminal writes | terminal-state precondition and state commit | result/error payload filenames and public methods |
| service optimization | prepared input and named pipeline stages | supplied-matrix bypass and dated-provider revalidation |
| compatibility environment entry points | raw-source parsers on the owning configuration types | public `from_env` wrappers remain for source compatibility |

## Review exceptions

- Exact DP and matching loops retain meaningful nesting. Extracting one-line helpers would obscure mask/frontier invariants and add hot-loop call/borrow complexity.
- Large files containing public wire/schema declarations and extensive in-file tests were not split solely to meet a line-count target.
- The storage facade remains one public module because splitting its public schema and traits would risk path churn; only genuinely identical transition logic was extracted.
- No serde/envy dependency was added. The existing alias, CSV, non-Unicode, warning, and exact error contracts require specialized handling; a small typed `EnvReader` reduced repetition without introducing a second mapping layer.
