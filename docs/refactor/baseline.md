# Refactor baseline

Recorded on 2026-11-01 (Asia/Seoul) before the structural work in this change.

## Repository and environment

- Starting HEAD: `671005889802e79f1c7b4d2c07ecf36fa39e6a3b`
- Branch: `impl`
- Platform inspected and tested: Windows, `x86_64-pc-windows-gnu`
- Rust: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- Cargo: `cargo 1.99.1 (5f94df478 2026-08-27)`
- Repository instructions: no `AGENTS.md` was present.
- The default Windows toolchain could not link because PATH selected the MSYS `link.exe`. Validation therefore uses the installed GNU host explicitly.

The worktree was already dirty. In particular, the user had replaced the old `spec/00_READ_FIRST.txt` through `spec/20_rollout_docs_config_and_followup.txt` set with the untracked `spec/00_START_HERE.txt` through `spec/14_SOURCE_AUDIT_AND_COMPATIBILITY_MAP.txt` and `spec/99_FILE_CHECKSUMS.txt`. Work on the preceding configuration request had also introduced local `main.rs`, `config`, and `server` changes. No reset, clean, stash, or history rewrite was performed, and the specification files were not edited by this implementation.

## Structural baseline

At HEAD, `src/main.rs` was about 400 lines and owned environment reads, defaulting, validation, provider/solver/storage construction, bind/serve, startup logging, and shutdown signal handling. Representative large production files were:

| File | HEAD lines | Main mixed responsibilities |
| --- | ---: | --- |
| `src/http.rs` | 1,507 | router facade, DTOs, errors, handlers, SSE |
| `src/service.rs` | 1,611 | request preparation through final response and remediation |
| `src/solver/orchestrator/runner.rs` | 1,629 | orchestration, two worker loops, candidate state |
| `src/storage.rs` | 1,110 | public schema, file I/O, transitions, recovery, timeline |
| `src/jobs.rs` | 901 | queue lifecycle, worker, progress, terminal persistence |
| `src/routing/matrix_builder.rs` | 348 | pair execution plus distributed shared state |
| `src/solver/heuristic/annealing/solver.rs` | 397 | setup plus all mutable search state |

The exact DP was already split across `bit_dp.rs`, `frontier.rs`, `transition.rs`, and `stats.rs`. Its nested mask/state transition loops are algorithmic, not accidental control-flow nesting, so they were retained.

No directly managed `mod.rs` existed under `src/`, `tests/`, or `benches/`; the requested file layout was already in place. The only explicit path attribute found was the intentional test fixture declaration in `tests/support.rs`.

## Compatibility baseline

The baseline contracts recorded before editing are expanded in [compatibility.md](compatibility.md). They include:

- top-level stderr prefix `troute server error: ` and success/failure exit codes;
- existing environment names, aliases, warning-and-default exceptions, and validators;
- the `/health`, `/optimize`, `/route/providers/policy`, and `/integration/*` routes;
- SSE event names, persisted snapshot authority, sequence IDs, heartbeat, and terminal closure;
- `data_dir/index.json` and per-job JSON/JSONL filenames;
- the public modules and re-exports from `src/lib.rs`.

## Baseline validation notes

- HEAD and the refactored tree were benchmarked from separate worktrees with the same GNU release toolchain, synthetic matrices, seed, and concurrency.
- No live provider, paid API, `.env`, user job data, or `.local/troute` data was used.
- `tests/http_api.rs::pending_queue_time_counts_toward_debug_minimum` is timing-sensitive: in the refactored full HTTP run it exceeded its `<250 ms` assertion once and passed immediately when isolated. The assertion and timeout were not changed.
