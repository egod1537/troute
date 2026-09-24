# Refactor migration and regression plan

This plan keeps module moves behavior-neutral. Each phase must start from a
green commit and end in its own green commit. A phase may move code and adjust
imports/re-exports, but it must not introduce a new algorithm, policy, default,
wire field, provider request, or error semantic.

## Required phase order

| Phase | Mechanical scope | Regression focus |
| --- | --- | --- |
| 0 | Confirm the current HEAD baseline | Record all commands and benchmark output before edits |
| 1 | Solver core | Public traits, types, objective ordering, errors, and re-exports |
| 2 | Exact bit DP | Route, metrics, frontier/pruning counts, brute-force agreement |
| 3 | Initial routes | Greedy/random/MST routes, fixed endpoints, directed final evaluation |
| 4 | Christofides/matching | Matching cost, Euler/shortcut route, approximation checks |
| 5 | Heuristics/orchestrator | Fixed-seed SA, candidate metadata/count/order, selected strategy |
| 6 | Routing/tcache | Pair request contract, bounded matrix assembly, timeout/cancel/errors |

Do not begin the next phase until the current phase passes every required gate.
If a behavior change is necessary, land it separately before or after the
mechanical refactor with its own tests and review.

## Gates for every phase

Run the complete gate with:

```sh
bash scripts/check_refactor_regression.sh
```

The script executes:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --test algorithm_validation
cargo test --test refactor_regression
cargo test --bench clustered_solver
```

The benchmark gate is correctness-oriented: compare selected strategy, route
metrics, state/frontier counts, iteration counts, and matching costs. Wall-clock
timings are informational and must not be used as an exact equality assertion.

## Locked behavior

`tests/refactor_regression.rs` is the cross-module compatibility baseline. It
uses only public APIs and locks:

- the exact route and objective metrics for a canonical directed matrix;
- deterministic SA output and move statistics for seed `42`;
- orchestrator selected strategy, candidate count/order, metrics, and metadata;
- caller-supplied matrix bypass of the routing provider;
- the serialized API response shape and objective values.

Existing focused suites remain authoritative for deeper invariants:

- `tests/algorithm_validation.rs`: exact/brute-force, pruning, matching, and
  directed route validation;
- routing and tcache unit tests: pair order, concurrency, deadline,
  cancellation, HTTP DTOs, and error messages;
- `tests/http_api.rs` and `tests/v0_contract.rs`: HTTP and v0 wire contracts;
- `benches/clustered_solver.rs`: deterministic solver and strategy baselines.

## Commit and compatibility rules

1. Keep one mechanical concern per commit; do not accumulate all phases into a
   single commit.
2. Do not combine file moves with algorithm or policy changes.
3. Preserve existing public paths with re-exports when implementations move.
4. Review `git diff --check` and confirm no generated benchmark output changed.
5. Record any intentionally changed baseline in a separate change explaining
   the reason and expected user-visible effect.
