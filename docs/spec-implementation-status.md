# Portfolio specification implementation status

This table tracks `spec/00` through `spec/20`. “External evidence pending” is
not treated as a measured result.

| Spec | Status | Evidence |
| --- | --- | --- |
| 01 | Complete | `docs/architecture/portfolio-scheduler.md` documents the legacy call graph, boundaries, data flow, capability matrix, deadlines, and state transitions. |
| 02 | Complete | `solver/portfolio/descriptor.rs` provides stable IDs, descriptors, typed Registry errors, availability and applicability. |
| 03 | Complete | `resources.rs` provides cancellation-aware finite waits and RAII slots/native-thread/process/memory reservations; every orchestrated strategy uses it. |
| 04 | Complete | Versioned bounded telemetry schema and central monotonic sequence collector in `telemetry.rs`. |
| 05 | Complete with honest null metrics | Existing solver metadata is promoted to run summaries; unsupported CPU/RSS remains null. LKH/SCIP native values are not simulated. |
| 06 | Complete | Shared evaluator remains the selection point; `BestSolutionTracker`, objective vector, fingerprint and bounded policy reward are implemented. |
| 07 | Complete | One orchestrator/executor/resource/telemetry path supports all eight policy IDs. Invalid options are typed errors. |
| 08 | Complete | CURRENT preserves exact-first legacy behavior; PARALLEL_ALL queues all applicable baselines under resource caps. |
| 09 | Complete | Seeded RR and weighted virtual slot-time policies; non-preemptible work is documented and not labelled paused. |
| 10 | Complete | Deterministic staged Racing/successive halving with warm-up and stable ties. |
| 11 | Complete | UCB1 and seeded Beta-posterior Thompson selection update only after a completed pull. |
| 12 | Complete | Input-only feature extraction, deterministic rule model/version and environment fallback metadata. |
| 13 | Complete | Optional response telemetry, append-only JSONL, cursor REST, solver SSE, capabilities endpoint, opt-in capped request override, and old-record fallback. |
| 14 | Complete | Testbed dynamic common Solver table plus existing plug-in details; unknown Solver IDs receive a generic view. |
| 15 | Complete | Testbed scheduling Gantt distinguishes wall time, logical slots, decisions, deadlines, Run IDs and real states. |
| 16 | Complete | Component-wise lexicographic anytime step chart; no weighted scalar or false optimality gap. |
| 17 | Complete | `portfolio_benchmark` fixes matrix/budget/slots/seed, records hashes/manifests/raw/timeline/summary artifacts, rotates policy order, and never uses live providers. |
| 18 | Complete | Testbed experiment dashboard imports measured JSON and aggregates feasible rate and wall p50/p95 by policy; empty data is explicit. |
| 19 | Complete for available repository checks | Portfolio contracts and existing regression suites cover correctness/concurrency/wire behavior, including 100K event pressure and unreachable required places across every policy. Exact commands and environment omissions are in `portfolio-validation-report.md`. |
| 20 | Complete | Default CURRENT/SUMMARY rollout, Compose variable forwarding, opt-in Docker feature build, benchmark/architecture docs, and rollback settings. |

## External evidence still pending

- A real LKH-3 binary run is not claimed. The executable is independently
  installed and its licence must be reviewed; unavailable status is explicit.
- SCIP feature-enabled compilation passed; a full native SCIP benchmark matrix remains operational evidence rather than a fabricated fixture.
- Docker/Compose and `/bin/bash` deployment checks could not run on this Windows host; see the validation report.
- Billable tcache/Google calls remain explicit ignored integration tests.
- Production CPU/RSS and long-duration p95 observations are operational data,
  not fabricated repository fixtures.
