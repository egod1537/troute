# Configuration

This reference covers the runtime settings previously documented in the root
README. [`../.env.example`](../.env.example) is the checked-in template for
local and Compose configuration.

When `travel_time_matrix` is omitted, optimization queries one tcache Route Job
for each directed pair, and `PairwiseMatrixRoutingProvider` assembles those
results into the troute-owned matrix. A caller-supplied matrix bypasses tcache
after its dimensions and zero diagonal are validated. `TCACHE_BASE_URL` is
required, and missing or invalid configuration fails application startup.

| Variable | Default | Description |
| --- | --- | --- |
| `TROUTE_PORT` | `8080` | HTTP server port; 1-65535 |
| `TROUTE_DATA_DIR` | `.local/troute` | persisted Job records and telemetry directory |
| `TCACHE_BASE_URL` | none | required tcache server base URL |
| `TCACHE_POLL_INTERVAL_MS` | `250` | positive Route Job status polling interval |
| `TCACHE_REQUEST_TIMEOUT_MS` | `30000` | positive total timeout for each pair query |
| `TCACHE_PAIR_CONCURRENCY` | `4` | maximum concurrent directed pair Route Jobs; 1-256 |
| `TCACHE_MATRIX_TOTAL_TIMEOUT_MS` | `120000` | total deadline for one matrix build in milliseconds; 1-86,400,000 |
| `TROUTE_MAX_CONCURRENT_JOBS` | `4` | maximum background Jobs executing concurrently; additional Jobs remain pending |
| `TROUTE_MAX_OUTSTANDING_JOBS` | `100` | maximum admitted non-terminal Jobs; excess submissions receive HTTP 429 |
| `TROUTE_MAX_CONCURRENT_MATRIX_PREVIEWS` | `2` | maximum simultaneous `/integration/matrix` requests; excess previews receive HTTP 429 |
| `TROUTE_MAX_ROUTING_PAIRS` | `10000` | maximum `N(N-1)` provider queries for a request without a supplied matrix |
| `ROUTE_PROVIDER` | `auto` | `auto` applies policy resolution; a provider name globally forces every routed request |
| `ROUTE_PROVIDER_POLICY_JSON` | built-in policy | country × mode policy JSON, parsed and validated at startup |
| `ROUTE_PROVIDER_OVERRIDE_ENABLED` | `false` | permits request-level `route_provider` only while `ROUTE_PROVIDER=auto` |
| `JAPAN_TRANSIT_PROVIDER` | none | deprecated compatibility fallback used only when JSON does not define JP + TRANSIT |
| `TROUTE_EXACT_LIMIT` | `15` | maximum location count at which the orchestrator also runs exact bit-DP; 1-15 |
| `TROUTE_MAX_EXACT_CLUSTER_SIZE` | `10` | requested intermediate locations per exact cluster; 1-15 (internally capped at 13 to reserve entry/exit anchors) |
| `SOLVER_MAX_CONCURRENCY` | `4` | maximum independently running solver strategies; 1-256 |
| `SOLVER_CPU_SLOTS` | `SOLVER_MAX_CONCURRENCY` | preferred logical CPU-slot limit; native thread reservations consume this capacity |
| `TROUTE_SOLVER_TOTAL_BUDGET_MS` | `30000` | global solver deadline shared by baseline and SA work |
| `SOLVER_SA_CHUNK_MS` | `250` | maximum time allocated to one anytime SA run |
| `SOLVER_DEADLINE_SAFETY_MS` | `50` | time reserved for schedule calculation and response assembly |
| `SOLVER_STRATEGY_TIMEOUT_MS` | `30000` | per-strategy safety cap, bounded by the global deadline |
| `SOLVER_SA_SEEDS` | `42` | deterministic SA base seed list, cycled by restart round before initializer-specific derivation |
| `TROUTE_REMEDIATION_BUDGET_MS` | `500` | separate post-failure budget for validated alternative-mode and removal probes |
| `TROUTE_REMEDIATION_MAX_REMOVE_PROBES` | `8` | maximum intermediate locations tested as removal candidates after one failure |
| `TROUTE_MAX_FAILURE_SUGGESTIONS` | `3` | maximum ranked, de-duplicated remediation suggestions returned per failure |
| `MATCHING_STRATEGY` | `auto` | Christofides benchmark/debug override: `auto`, `bitdp`, or `blossom` |
| `MATCHING_BIT_DP_THRESHOLD` | `20` | auto-policy odd-vertex threshold, from 0 through 20 |
| `SOLVER_ENABLE_MIP_SCIP` | `false` | enable SCIP MIP; the binary must be built with `solver-mip-scip` |
| `SOLVER_MIP_SCIP_TIMEOUT_MS` | `30000` | SCIP strategy cap, also bounded by the shared solver deadline |
| `SOLVER_MIP_SCIP_THREADS` | `1` | SCIP native thread limit |
| `SOLVER_MIP_SCIP_MEMORY_MB` | `1024` | SCIP native memory limit in MiB |
| `SOLVER_ENABLE_LKH3` | `false` | enable the separately installed LKH-3 executable adapter |
| `LKH3_EXECUTABLE` | none | path to an operator-supplied LKH-3 executable; required when enabled |
| `SOLVER_LKH3_TIMEOUT_MS` | `30000` | LKH-3 process cap, also bounded by the shared solver deadline |
| `SOLVER_LKH3_SEED` | `42` | deterministic LKH-3 seed |
| `SOLVER_COMPARE_ALL` | `false` | run all enabled strategies for comparison instead of early exact selection |
| `SOLVER_INCUMBENT_EXCHANGE` | `false` | reserved experiment; rejected until a real LKH benchmark demonstrates benefit |
| `SOLVER_PORTFOLIO_POLICY` | `CURRENT` | `CURRENT`, `PARALLEL_ALL`, `ROUND_ROBIN`, `WEIGHTED_FAIR`, `RACING`, `UCB1`, `THOMPSON_SAMPLING`, or `INSTANCE_AWARE` |
| `SOLVER_POLICY_SEED` | `42` | per-Job deterministic policy RNG seed |
| `SOLVER_POLICY_MIN_QUANTUM_MS` | `250` | minimum useful restartable-arm quantum |
| `SOLVER_RACING_WARMUP_RUNS` | `1` | observations per active arm before the first Racing elimination |
| `SOLVER_UCB_EXPLORATION` | `1.41421356237` | positive UCB1 exploration coefficient |
| `SOLVER_MIN_EXPLORATION_RATIO` | `0.1` | bounded exploration floor shared by adaptive policies |
| `SOLVER_MAX_STARVATION_RUNS` | `8` | completed pulls before a runnable arm receives starvation protection |
| `SOLVER_POLICY_WEIGHTS_JSON` | empty | positive solver/arm weights for weighted fair scheduling |
| `SOLVER_TELEMETRY_MODE` | `SUMMARY` | `OFF`, `SUMMARY`, or opt-in `DETAILED` trace collection |
| `SOLVER_TELEMETRY_MAX_EVENTS` | `10000` | bounded telemetry events retained per Job; maximum 100000 |
| `SOLVER_MAX_EXTERNAL_PROCESSES` | `1` | concurrent external solver child-process reservations |
| `SOLVER_MEMORY_LIMIT_MB` | none | optional aggregate scheduler memory hint limit; not a measured RSS value |
| `TROUTE_PORTFOLIO_EXPERIMENT_ENABLED` | `false` | permits capped optional `portfolio_experiment` request overrides |

Portfolio configuration migration:

| Previous setting | Current setting | Compatibility |
| --- | --- | --- |
| `SOLVER_MAX_CONCURRENCY` | `SOLVER_CPU_SLOTS` | The old name remains the fallback when the preferred variable is absent. |
| no scheduler-policy setting | `SOLVER_PORTFOLIO_POLICY=CURRENT` | `CURRENT` preserves the prior exact-first/baseline/SA path. |
| terminal candidate diagnostics only | `SOLVER_TELEMETRY_MODE=SUMMARY` | Legacy `/optimize` JSON remains unchanged; versioned telemetry uses separate Job endpoints. |

Rollback does not require deleting persisted Jobs: set
`SOLVER_PORTFOLIO_POLICY=CURRENT`, `SOLVER_TELEMETRY_MODE=SUMMARY`, and
`TROUTE_PORTFOLIO_EXPERIMENT_ENABLED=false`; disable optional SCIP/LKH flags,
then restart the service. Existing telemetry files remain diagnostic data and
are not executed or automatically deleted.

Provider resolution uses this order: a non-`auto` `ROUTE_PROVIDER` global
force; otherwise an enabled request `route_provider` override, country + mode,
country default, mode default, and global default. The built-in policy selects
Google for JP driving/walking/bicycling, Ekispert for JP transit, Kakao Mobility
for KR driving, Kakao Maps for the other KR modes, and Google globally. For
example:

```sh
ROUTE_PROVIDER_POLICY_JSON='{"countries":{"JP":{"modes":{"TRANSIT":"ekispert"}}},"modeDefaults":{"TRANSIT":"google"},"defaultProvider":"google"}'
```

Country keys are normalized to uppercase and must be ISO 3166-1 alpha-2 codes.
Modes and provider names are strict. Invalid JSON, unknown values, unavailable
providers, and unsupported provider capabilities fail explicitly; selection
never silently falls back after a policy match. Registered provider names are
`google`, `kakao-mobility`, `kakao-maps`, `ekispert`, `navitime`, and `otp`.
`JAPAN_TRANSIT_PROVIDER` is deprecated and only fills a missing JP transit rule;
an explicit JSON rule takes precedence.

`GET /route/providers/policy` returns the effective policy, provider
capabilities, availability, and flattened country/mode routes without secrets.
The testbed reaches it through its `/api` proxy and renders that response
directly. Selected provider, selection
source/reason, normalized country, and mode are returned with routed Job results.
Only the selected provider is forwarded to tcache; the policy itself is not part
of downstream cache identity.

`GET /integration/solver-capabilities` returns versioned Solver descriptors,
availability reasons, the eight policy descriptors, and server experiment
limits without executable paths or environment values. Completed and running
Job telemetry is available from
`GET /integration/jobs/{job_id}/solver-telemetry?after=<sequence>&limit=<n>`;
`GET /integration/jobs/{job_id}/solver-events` provides the corresponding SSE
stream. Old Jobs without telemetry return an explicit empty, uncollected page.

The adapter creates `POST /api/route/jobs` with two Place IDs, polls the returned
Job, reads `result.routes[0].durationSeconds`, and converts seconds to whole
minutes by rounding up. Directed pairs run with bounded concurrency. Each pair
uses the earlier of its own timeout and the remaining matrix deadline. A pair
failure or total timeout stops unscheduled work and cooperatively cancels
in-flight tcache Jobs. Matrix errors report timeout classification, elapsed
time, completed/total pair counts, and the failed directed pair.
It also attempts the tcache Route Job cancel endpoint when a pair query times
out. The deprecated `TCACHE_MATRIX_POLL_INTERVAL_MS` and
`TCACHE_MATRIX_TIMEOUT_MS` names remain fallback aliases. A request may include
`travel_date` and an IANA `time_zone`; they must be supplied together. The
initial matrix uses that date and the selected start time. After selecting a
route, provider-routed dated requests re-query every selected leg once at its
calculated departure instant and recalculate the schedule with the corrected
arcs. This bounded pass never loops. Its counts, maximum delta, applied status,
schedule feasibility, and any transparent fallback warning are returned in
`time_dependent_revalidation`. Undated requests retain the current-UTC-plus-five-
minute compatibility behavior. Caller-supplied matrices skip provider
revalidation because their travel times are explicit.

## Optional external solvers

The default build has no native solver dependency. To compile and run the SCIP
strategy, enable the opt-in feature; `russcip` builds a bundled SCIP 10.0.2:

```sh
cargo test --features solver-mip-scip
SOLVER_ENABLE_MIP_SCIP=true cargo run --features solver-mip-scip
```

The adapter enforces the shared deadline and the configured native time, thread,
and memory limits. It reports native status, proof status, best bound, relative
gap, thread count, and source version through normal solver candidate metadata.
If `SOLVER_ENABLE_MIP_SCIP=true` is used with a binary built without the feature,
startup fails explicitly.

LKH-3 is intentionally not downloaded or redistributed by this repository.
Install it separately after reviewing its licence, then opt in with an explicit
path:

```sh
SOLVER_ENABLE_LKH3=true \
LKH3_EXECUTABLE=/absolute/path/to/LKH \
cargo run
```

troute writes a temporary problem/parameter pair, invokes the process without a
shell, parses the output tour, validates it with the common feasibility and
objective code, and removes the temporary files. Cancellation and deadline
expiry terminate the child process. Partial-visit optimization remains in the
shared solver pipeline; LKH is used only to improve a fixed selected subset.

`SOLVER_INCUMBENT_EXCHANGE` is deliberately unavailable: spec 19 requires real
LKH benchmark evidence before that experiment may be enabled. SCIP and LKH can
still run independently under the same bounded orchestrator.
## Secrets

`.env` and `.env.*` are excluded from Git and the Docker build context;
`.env.example` contains only safe defaults. Do not commit keys or tokens.
Google credentials belong to tcache and are not configured in troute.
