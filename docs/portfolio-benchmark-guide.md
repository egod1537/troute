# Portfolio benchmark guide

`portfolio_benchmark` compares scheduler policies using generated local
matrices. It never calls tcache or a paid routing provider. Every invocation
creates a unique experiment directory and leaves prior results untouched.

## Smoke run

```sh
cargo run --release --bin portfolio_benchmark -- \
  --output-dir benchmark-results \
  --policies CURRENT,PARALLEL_ALL \
  --seeds 1,2 \
  --sizes 5,10 \
  --scenarios wide,asymmetric \
  --budgets-ms 1000 \
  --slots 1,2
```

## Full local profile

Use at least ten seeds and keep the machine otherwise idle:

```sh
cargo run --release --bin portfolio_benchmark -- \
  --output-dir benchmark-results \
  --policies CURRENT,PARALLEL_ALL,ROUND_ROBIN,WEIGHTED_FAIR,RACING,UCB1,THOMPSON_SAMPLING,INSTANCE_AWARE \
  --seeds 1,2,3,4,5,6,7,8,9,10 \
  --sizes 5,10,15,20,30,50 \
  --scenarios wide,tight,asymmetric,sparse,zero-arc,partial,mixed-priorities,required-infeasible \
  --budgets-ms 1000,5000,30000 \
  --slots 1,2,4
```

The full Cartesian profile is intentionally opt-in and can take hours. Policy
order rotates by instance/seed to reduce consistent cache and thermal bias.

## Artifacts

- `manifest.json`: Git/Rust/OS/arch/CPU concurrency, feature flags, complete
  experiment configuration, and external solver availability.
- `runs.jsonl`: one bounded policy run with problem/matrix hashes, configured
  budget, terminal status, objective, proof metadata, and telemetry summary.
- `timelines.jsonl`: best-so-far and scheduler events.
- `summary.json` and `summary.csv`: feasible rate, wall p50/p95, proof count,
  decision count, nominal reserved slot-time, and CPU measurement coverage.

CPU/RSS values are reported only when measured. Nominal slot-ms is not CPU
time. For `N <= 15`, a validated Exact result can be an oracle. Larger-case
comparisons are best-known comparisons and must not be labelled optimality
gaps. SCIP bounds apply to the recorded objective phase; they are not a global
multi-phase proof by themselves.

LKH-3 is not downloaded or simulated. If `LKH3_EXECUTABLE` is unavailable, the
manifest records it as unavailable and no LKH performance value is fabricated.
SCIP likewise requires the opt-in Cargo feature and runtime switch.
