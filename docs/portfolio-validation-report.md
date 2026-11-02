# Portfolio validation report

This report separates repository checks from optional external evidence.

## Default implementation

The validation gate is:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets -- --test-threads=1
cd testbed && npm run build && npm test
```

Validated on Windows GNU Rust in this workspace:

- `cargo fmt --all -- --check`: passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo test --all-targets -- --test-threads=1`: passed (249 tests passed; one live,
  billable tcache integration test ignored by design).
- `cargo test --features solver-mip-scip --all-targets -- --test-threads=1`:
  passed (251 tests passed, including both SCIP solver tests; the same live
  tcache integration test was ignored by design).
- `cd testbed && npm run build`: passed.
- `cd testbed && npm test`: passed (34 Playwright tests).

Contract coverage includes stable Solver/Run IDs, eight policy wire values,
seeded adaptive arm decisions and starvation protection, panic-safe RAII slot
release, 100K-event pressure sampling, terminal pairing, JSONL cursor reads, a
no-provider benchmark smoke run, API compatibility, partial visits,
sparse/zero arcs, unreachable required places under all policies,
cancellation, and existing exact/orchestrator tests.

## Optional checks

```sh
cargo test --features solver-mip-scip --all-targets -- --test-threads=1
```

The feature-enabled test suite passed in this environment; a complete SCIP
benchmark matrix was not run. A real LKH run requires separate installation
and licence review. Billable
tcache/Google integration and a real LKH benchmark are not part of the default
test suite and are never represented by generated values.

Docker/Compose validation was not executable because the `docker` command is
not installed in this Windows environment. The macOS/Linux deployment-script
unit suite was also not executable because it invokes `/bin/bash`; this host
only provides MSYS/Git Bash paths. These are environment omissions, not passing
results.

## Known platform boundary

In-process Rust/native threads are cooperatively cancelled. Rust cannot safely
terminate an arbitrary non-cooperative native thread, so the global deadline
includes a safety margin and this limitation is exposed rather than described
as pause/resume. External LKH children are explicitly terminated on timeout or
cancellation.
