# Module map

The intended dependency direction is HTTP/Jobs → Service → Routing/Solver →
Schedule, with API conversion at the boundary and storage independent from the
optimization algorithms.

| Module | Owns | Important types and entry points |
| --- | --- | --- |
| `main.rs` | Runtime configuration and dependency wiring | `run` |
| `api.rs` | JSON request/response contract and domain conversion | `OptimizeRouteRequest`, `OptimizeRouteResponse` |
| `http.rs` | Shared HTTP state, error mapping, and module facade | `ApiError`, `AppState`, public router re-exports |
| `http/router.rs` | Axum route construction and admission limits | `router_with_storage`, `router_with_storage_options` |
| `http/handlers/` | Endpoint groups | `optimization`, `matrix`, `diagnostics`, `health`, `jobs`, and `events` handler entry points |
| `jobs.rs` | Background execution lifecycle and module facade | `JobRunner`, `JobExecutor`, `submit_with_completion`, `run_inner` |
| `jobs/cancellation.rs` | Active-Job cancellation registration | `CancellationRegistry` |
| `storage.rs` | Durable Job state and terminal payloads | `JobStore`, `FileJobStore`, `StoredJob`, `JobState` |
| `service.rs` | Provider-to-response use case | `RouteOptimizationService::optimize_with_cancellation` |
| `domain.rs` | Solver-facing business values | `OptimizationProblem`, `Location`, `TimeOfDay`, `StartPolicy` |
| `routing/` | Provider abstraction, matrix construction, provider policy | `RoutingProvider`, `PairwiseMatrixRoutingProvider`, `RoutingContext` |
| `providers/tcache/` | tcache HTTP adapter and polling | `TcacheTravelTimeProvider` |
| `matrix.rs` | Directed travel-time matrix invariant | `TravelTimeMatrix` |
| `solver/` | Solver contract, objective, exact/heuristic implementations | `SolverInput`, `SolverSolution`, `RouteSolver`, `SolverOrchestrator` |
| `solver/portfolio.rs`, `solver/portfolio/` | Registry, resource leases, policy state, objective tracking, telemetry | `SolverRegistry`, `ResourceManager`, `RestartableArmSelector`, `BestSolutionTracker`, `TelemetryCollector` |
| `bin/portfolio_benchmark.rs` | Reproducible local policy experiments | manifests, raw JSONL, timelines, summaries |
| `schedule.rs` | Minute-accurate route materialization | `RoutePlan`, `calculate_schedule_from` |
| `events.rs` | Pipeline progress vocabulary | `ProgressStage`, `OptimizationEventReporter` |
| `cancellation.rs` | Cooperative cancellation primitive | `CancellationToken` |
| `failure.rs` | Failure classification/remediation suggestions | `FailureDetail`, `FailureSuggestion` |
| `observation.rs` | Developer-facing request timeline | `JobObservationRecorder` |

## Principal data flow

1. `OptimizeRouteRequest` is deserialized by a handler in `http/handlers/`.
2. `api.rs` validates and converts it to `OptimizationProblem`.
3. A supplied matrix or `routing/` produces `TravelTimeMatrix`.
4. The service builds `SolverInput { problem, matrix, cancellation }`.
5. A solver returns `SolverSolution` (a visit order) plus optional diagnostics.
6. `schedule.rs` converts the order and selected start into `RoutePlan`.
7. `api.rs` maps the plan to `OptimizeRouteResponse`.
8. For Job requests, `jobs.rs` persists progress and the terminal response in
   `storage.rs`; HTTP readers never reconstruct status from solver events.

Provider adapters know about remote routing protocols, but solvers only depend
on the matrix. Storage knows serialized requests and results, but algorithms do
not depend on storage or HTTP. The schedule layer consumes domain values, the
matrix, and a solver order and is the last feasibility check before output.

## Suggested code-reading path

Start with [`src/main.rs`](../../src/main.rs), then follow
[`src/http.rs`](../../src/http.rs) into [`src/jobs.rs`](../../src/jobs.rs) and
[`src/service.rs`](../../src/service.rs). Matrix acquisition is in
[`src/routing`](../../src/routing); optimization starts at
[`src/solver.rs`](../../src/solver.rs) and production orchestration is in
[`src/solver/orchestrator/runner.rs`](../../src/solver/orchestrator/runner.rs).
Finish with [`src/schedule.rs`](../../src/schedule.rs) and
[`src/storage.rs`](../../src/storage.rs).

