# Compatibility map

This map describes the contracts preserved by the structural refactor. Defaults are taken from the existing `Default` implementations and exported constants rather than copied into a second runtime table.

## Environment configuration

| Keys | Default/source | Parsing and compatibility rule |
| --- | --- | --- |
| `TROUTE_PORT` | `8080` | integer `1..=65535`; invalid values fail startup |
| `TROUTE_DATA_DIR` | `.local/troute` | trim is used only to reject an empty value; the original path string is retained |
| `TCACHE_BASE_URL` | absent | optional valid `http`/`https` URL; absent or blank disables provider routing, while invalid non-blank values retain the `invalid tcache configuration:` error boundary |
| `TCACHE_POLL_INTERVAL_MS`, legacy `TCACHE_MATRIX_POLL_INTERVAL_MS` | `250` ms | positive `u64`; new key wins even when invalid, so there is no fallback from an invalid new value |
| `TCACHE_REQUEST_TIMEOUT_MS`, legacy `TCACHE_MATRIX_TIMEOUT_MS` | `30000` ms | same alias and positive-value rules |
| `TCACHE_PAIR_CONCURRENCY` | routing constant | integer `1..=256` |
| `TCACHE_MATRIX_TOTAL_TIMEOUT_MS` | routing constant | integer `1..=86400000` ms |
| `ROUTE_PROVIDER` | `auto` | global force or automatic policy selection |
| `ROUTE_PROVIDER_POLICY_JSON` | built-in policy | blank means built-in; nonblank JSON is parsed and registry-validated |
| `ROUTE_PROVIDER_OVERRIDE_ENABLED` | `false` | only case-insensitive `true`/`false` and `1`/`0` |
| `JAPAN_TRANSIT_PROVIDER` | absent | deprecated JP transit fallback unless the JSON policy explicitly owns that route; emits one startup warning |
| `TROUTE_EXACT_LIMIT` | `SolverOrchestratorConfig::default()` | integer `1..=EXACT_MAX_LOCATIONS` |
| `TROUTE_MAX_EXACT_CLUSTER_SIZE` | solver default | integer `1..=MAX_EXACT_CLUSTER_SIZE`, followed by existing clustered validation |
| `SOLVER_CPU_SLOTS`, `SOLVER_MAX_CONCURRENCY` | solver default (`4`) | CPU slots has priority when present; integer `1..=256` |
| `SOLVER_STRATEGY_TIMEOUT_MS`, `TROUTE_SOLVER_TOTAL_BUDGET_MS`, `SOLVER_SA_CHUNK_MS`, `SOLVER_DEADLINE_SAFETY_MS` | solver defaults | positive `u64` milliseconds; safety margin must remain below total budget |
| `SOLVER_SA_SEEDS` | `[42]` | comma-separated `u64`, item trim retained; order retained; the existing validator rejects empty or duplicate lists |
| `MATCHING_STRATEGY` | `auto` | `auto`, `bitdp`, or `blossom` |
| `MATCHING_BIT_DP_THRESHOLD` | matching constant | integer `0..=MAX_BIT_DP_MATCHING_VERTICES` |
| `SOLVER_ENABLE_MIP_SCIP`, `SOLVER_ENABLE_LKH3`, `SOLVER_COMPARE_ALL`, `SOLVER_INCUMBENT_EXCHANGE`, `TROUTE_PORTFOLIO_EXPERIMENT_ENABLED` | solver defaults | common bool parser preserves case-insensitive `true/false`, `1/0`, `yes/no`, `on/off` |
| `SOLVER_MIP_SCIP_TIMEOUT_MS`, `SOLVER_LKH3_TIMEOUT_MS`, `SOLVER_LKH3_SEED` | solver defaults | positive `u64` |
| `SOLVER_MIP_SCIP_THREADS` | `1` | integer `1..=256`, then checked against logical CPU slots |
| `SOLVER_MIP_SCIP_MEMORY_MB` | `1024` | integer `1..=1000000` |
| `LKH3_EXECUTABLE` | absent | nonempty OS path; non-Unicode path remains supported through `var_os` |
| `SOLVER_PORTFOLIO_POLICY` | `CURRENT` | existing case-insensitive `PortfolioPolicy` parser |
| `SOLVER_POLICY_SEED` | `42` | positive `u64` |
| `SOLVER_POLICY_MIN_QUANTUM_MS` | SA chunk | positive `u64` |
| `SOLVER_RACING_WARMUP_RUNS` | `1` | integer `1..=10000` |
| `SOLVER_UCB_EXPLORATION` | `sqrt(2)` | finite floating point in `(0, f64::MAX]` |
| `SOLVER_MIN_EXPLORATION_RATIO` | `0.1` | finite floating point in `(0, 1]`, preserving the prior reader boundary |
| `SOLVER_MAX_STARVATION_RUNS` | `8` | positive `u64` |
| `SOLVER_POLICY_WEIGHTS_JSON` | empty map | blank/absent is empty; otherwise a JSON object, followed by existing positive finite weight validation |
| `SOLVER_TELEMETRY_MODE` | `SUMMARY` | `OFF`, `SUMMARY`, or `DETAILED`, case-insensitive |
| `SOLVER_TELEMETRY_MAX_EVENTS` | `10000` | integer `1..=100000` |
| `SOLVER_MAX_EXTERNAL_PROCESSES` | `1` | integer `1..=64` |
| `SOLVER_MEMORY_LIMIT_MB` | absent | positive `u64`, converted with the existing saturating MiB calculation |
| `TROUTE_REMEDIATION_BUDGET_MS` | `500` | positive `u64` milliseconds |
| `TROUTE_REMEDIATION_MAX_REMOVE_PROBES` | `8` | integer `1..=500` |
| `TROUTE_MAX_FAILURE_SUGGESTIONS` | `3` | integer `1..=100` |
| `TROUTE_MAX_CONCURRENT_JOBS` | `4` | positive `usize`; invalid/non-Unicode values warn and use the existing default rather than failing startup |
| `TROUTE_MAX_OUTSTANDING_JOBS` | `100` | positive `usize`; invalid value warns and defaults; unreadable/absent defaults silently |
| `TROUTE_MAX_CONCURRENT_MATRIX_PREVIEWS` | `2` | same positive-limit policy |
| `TROUTE_MAX_ROUTING_PAIRS` | `10000` | positive `usize`; invalid value warns and defaults; supplied matrices bypass this pair limit |

`AppConfig::from_env()` is the sole production aggregation boundary. Compatibility `from_env` methods and old router constructors remain public where they already existed, but production startup passes explicit validated configuration and handlers do not read the environment.

## Provider selection order

The resolver still applies: global forced provider, allowed request override, country+mode rule, country default, mode default, global default. The deprecated JP rule is marked as a legacy selection source and is not allowed to overwrite an explicit JSON country+mode rule. Unsupported or unavailable providers remain explicit errors rather than hidden fallbacks.

## HTTP routes

| Method | Path |
| --- | --- |
| GET | `/health` |
| GET | `/route/providers/policy` |
| GET | `/integration/solver-capabilities` |
| POST | `/optimize` |
| GET, POST | `/integration/jobs` |
| POST | `/integration/matrix` |
| GET | `/integration/jobs/{job_id}` |
| GET | `/integration/jobs/{job_id}/solver-telemetry` |
| GET | `/integration/jobs/{job_id}/solver-events` |
| GET | `/integration/jobs/{job_id}/events` |
| POST | `/integration/jobs/{job_id}/cancel` |
| GET | `/integration/jobs/{job_id}/timeline` |

The 1 MiB body limit, JSON error envelope fields, status/code/message mapping, unknown route/method behavior, request aliases/defaults, and `/api`-prefix absence are unchanged. `http.rs` remains the public router facade; handlers, SSE transport, router assembly, and error adaptation now have separate internal modules.

## SSE and jobs

- Subscription still occurs before the authoritative persisted snapshot is read.
- Event names and `id = sequence` remain unchanged; envelopes retain `sequence`, `updated_at`, state, and conditional result/error fields.
- Heartbeats, cache headers, lag recovery, terminal reconnect behavior, and close-after-terminal behavior are unchanged.
- `JobRunner` keeps semaphore permits and cancellation registry entries for the same lifecycle. A terminal event is emitted only after `TerminalWriteOutcome::Applied`.

## Disk layout

The storage schema and paths are unchanged:

- `data_dir/index.json`
- `data_dir/jobs/{job_id}/request.json`
- `state.json`, `result.json`, `error.json`
- `event-sequence.json`
- `solver-telemetry.jsonl`
- `timeline.jsonl`

Atomic JSON writes, append-only JSONL behavior, lock scope, terminal transition rules, and interrupted-job recovery retain their prior order and semantics. No migration or checkpoint-resume format was introduced.

## Public Rust API

The public modules and root re-exports in `src/lib.rs` remain in place. Existing router constructors remain available, and the explicit `router_with_storage_runtime_options` entry point is additive. Existing `TcacheRoutingConfig::from_env`, `RouteProviderPolicyResolver::from_env`, and `MatchingStrategyConfig::from_env` remain compatibility wrappers around shared parsing/validation logic. No Cargo edition, MSRV, dependency, or feature change was made.
