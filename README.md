# troute

troute is a travel itinerary optimization engine written in Rust.

## Overview

troute builds a feasible day itinerary from a set of places and a directed
travel-time matrix. It chooses the visit order, accounts for travel time,
respects each place's time window, and includes the requested stay duration
when calculating arrival and departure times. The first and last locations are
fixed endpoints; intermediate locations may be reordered or, with the optional
best-effort policy, skipped when the full itinerary is infeasible.

This is different from ordinary shortest-path routing. A routing engine finds
the path or travel time between two places. troute uses those pairwise travel
times to decide which places to visit, in what order, and when, while checking
the constraints of the complete itinerary.

## Features

- Itinerary optimization with fixed endpoints, time windows, stay durations,
  directed travel times, and optional visits.
- Asynchronous Jobs with persisted status, SSE updates, and cancellation.
- Travel-time lookup through tcache or caller-supplied dense and sparse matrices.
- A bounded solver portfolio with shared validation and deterministic selection.

## Optimization Strategies

troute runs a bounded portfolio of exact and heuristic solvers based on problem
size, input constraints, and available execution time.

| Strategy | Role | Availability |
| --- | --- | --- |
| Exact DP | Solves applicable small instances and proves optimality under troute's configured objective. | Built in. The default limit is 15 locations. |
| MIP (SCIP) | Adds a mixed-integer candidate with native proof and bound metadata. | Optional Cargo feature and runtime switch. SCIP is bundled through `russcip`. |
| LKH-3 | Produces an ATSP candidate for a fixed selected set. troute discards it if common schedule validation fails. | Optional, separately installed executable. |
| Heuristics | Greedy, MST Double-Tree, Christofides, clustering, and simulated annealing provide candidates for larger instances. | Built in. Symmetric TSP guarantees do not apply to troute's constrained problem. |

With the default `CURRENT` policy, Exact DP runs first when applicable and may
finish the search immediately. Otherwise, applicable baseline strategies run
under bounded concurrency and simulated annealing uses the remaining budget in
short restartable chunks. Other portfolio scheduling policies are available
for controlled experiments.

Every returned route is checked by the same evaluator. Selection favors
feasible routes, more optional visits, and better schedules under the requested
start policy. Solver-reported scores cannot bypass this step. See
[solver execution](docs/architecture/solver-execution.md) and the
[portfolio scheduler](docs/architecture/portfolio-scheduler.md) for the exact
ranking and execution model.

## How It Works

```mermaid
flowchart LR
    A[Request] --> B[Travel Time Matrix]
    B --> C[Solver Portfolio]
    C --> D[Schedule]
    D --> E[Response]
```

1. **Request**: The HTTP API validates the locations, fixed endpoints, time
   windows, stay durations, visit policy, and start policy.
2. **Travel Time Matrix**: troute validates a supplied directed matrix, builds
   one from pairwise tcache Route Jobs, or estimates one locally from coordinates
   when `use_euclidean_matrix` is enabled.
3. **Solver Portfolio**: Applicable exact, external, and heuristic strategies
   run within the configured resource and time budgets.
4. **Schedule**: The selected visit order is recalculated against the matrix and
   checked for all time-window and stay-duration constraints.
5. **Response**: The Job store persists the terminal result, diagnostics, and
   optional solver telemetry for polling or streaming clients.

## Getting Started

Install Rust 1.87 or later, then build and run the server:

```sh
cargo build
TCACHE_BASE_URL=http://127.0.0.1:1 cargo run
```

The server listens on port `8080` by default. Verify it from another terminal:

```sh
curl http://127.0.0.1:8080/health
```

Submit the self-contained matrix example and read its Job record:

```sh
curl -i -X POST http://127.0.0.1:8080/integration/jobs \
  -H 'Content-Type: application/json' \
  --data-binary @docs/examples/basic-request.json

curl http://127.0.0.1:8080/integration/jobs/readme-matrix-example
```

The completed route is `hotel -> park -> museum -> station` with 35 minutes of
travel. Change `job_id` in the example file before submitting it again.

The placeholder tcache URL is sufficient for requests that supply their own
travel-time matrix. See [Getting Started](docs/getting-started.md) for the
self-contained API example and [Configuration](docs/configuration.md) for
routing-provider and optional solver setup.
