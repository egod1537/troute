# Standalone itinerary validation

By default, `POST /validate` checks the submitted current visit order without running
a solver, creating a Job, persisting a result, or changing locations or times.
The existing optimization routes remain unchanged.

## Bounded existence check

Set `check_feasibility: true` beside `route` and send `service_start_times: {}` to
check whether any valid visit order exists under the optimization request's
constraints. Displayed service times are not additional fixed appointments in
this mode. The default current-order mode below retains its existing behavior.

The limit is **15 locations, including both endpoints**. Above this limit the
HTTP handler returns `UNSUPPORTED` before acquiring matrix capacity, querying a
provider or attempting greedy/DP search. This does not disable optimization.

For supported inputs, the engine first generates a deterministic nearest-neighbor
greedy order and checks it with the common schedule evaluator. A valid greedy
witness returns immediately. Otherwise an exact bitmask DP stores only the
earliest departure for each `(visited set, last location)`, reconstructing and
returning the first valid terminal order. Only exhaustion of this DP establishes
infeasibility. The DP uses `O(n^2 * 2^n)` time and `O(n * 2^n)` space; it does not
build objective/Pareto frontiers or run optimization's portfolio.

Both stages retain fixed endpoints, absolute locked positions, directed or
unreachable arcs, minute-accurate waiting/stays, closing times and required/optional
visit policies. For `EARLIEST`/`LATEST`, existence is checked at the minimum allowed
start: an earlier departure can wait to reproduce a later route. No latest-start
optimization is performed. The same earliest-time DP is reused by the exact
optimizer's latest-start feasibility probes.

The response adds `feasibility` only in existence mode:

| `status` | `method` | `schedule_feasible` | `visit_order` |
| --- | --- | --- | --- |
| `FEASIBLE` | `GREEDY` or `BIT_DP` | `true` | A validated witness, using location IDs |
| `INFEASIBLE` | `BIT_DP` | `false` | Empty |
| `UNSUPPORTED` | `null` | `null` | Empty |

Metadata also contains `location_count` and `max_locations`. `stops` contains the
witness schedule for feasible inputs; the submitted request is never edited.
An infeasible input produces a `NO_FEASIBLE_ROUTE` warning. Valid requests in all
three states leave `blocks_optimization=false`. Unsupported inputs use
`matrix_source: "none"`; other results describe the matrix actually checked.
Invalid domain inputs still return the existing invalid-input response.
Cancellation, provider failures and HTTP timeouts remain errors, never proofs of
infeasibility. No Job, billing action, remediation search or persisted result is
created by an existence check.

Verification includes greedy success, a greedy trap repaired by DP, locked-position
infeasibility, sparse directed arcs, optional visits, the 15/16 boundary and
cancellation. For 120 varied small instances, existence and returned witnesses
are cross-checked against exhaustive permutations using the common evaluator.
The actual troute binary and Trasolve gateway were exercised together for all
three statuses, both methods, legacy validation and the partial-visit capability
switch, with supplied matrices and no billable provider requests.

## Request

```json
{
  "route": {
    "job_id": "validation-correlation-id",
    "start_policy": "FIXED",
    "start_time": "09:00",
    "locations": [
      {"id": "a", "place_id": "a", "open_time": "09:00", "close_time": "18:00", "stay_minutes": 20},
      {"id": "b", "place_id": "b", "open_time": "09:00", "close_time": "18:00", "stay_minutes": 20}
    ],
    "travel_time_matrix": [[0, 10], [10, 0]]
  },
  "service_start_times": {"b": "09:10"}
}
```

`route` uses the optimize request fields, but its location array is the current
visit order. `job_id` is a correlation value only and is never stored. Optional
`service_start_times` maps location IDs to requested visit/service-start times;
unknown IDs are invalid input. These clock times may use single-minute precision.
The request/domain constraints on location opening times and stay durations are
shared with optimization. With no explicit start time the existing domain default
is midnight; this endpoint never searches EARLIEST/LATEST starts or changes order.
Clients should send the current start time explicitly.

Location closing times also accept `23:59` for an unrestricted end-of-day window.
Opening times and fixed route starts still require ten-minute alignment. A client
validating minute-precision service times can round its arrival/start down to the
preceding ten-minute boundary and retain the exact first visit time in
`service_start_times`; the simulator then accounts for the intervening wait.

## Response

HTTP 200 returns `input_valid`, nullable `schedule_feasible`,
`blocks_optimization`, `matrix_source`, `issues`, and `stops`.

- Invalid domain input returns `input_valid=false`, `schedule_feasible=null`,
  `blocks_optimization=true`, and an `INVALID_REQUEST` issue before routing I/O.
- A valid request is simulated in order, respecting waits, opening hours and stay
  duration through the same `schedule::visit_timing` used by optimization.
- `FIXED_VISIT_CONFLICT`, `TIME_WINDOW_VIOLATION`, `UNREACHABLE_ARC` and
  `OUTSIDE_SINGLE_DAY` are current-plan warnings. They set
  `schedule_feasible=false` but leave `blocks_optimization=false`: changing order
  or times may repair the plan. This is not a proof that no optimized route exists.
- Every issue includes severity, blocking status, optional location ID, message,
  failure details and suggestions where the shared failure logic can derive them.
  Fixed-time conflicts include the requested and earliest possible service times.
- Independent violations continue to be collected. Predicted times are read-only;
  the requested time is retained separately. After an unreachable arc or day
  overflow, suffix times are null instead of fabricated.
- `matrix_source` is `euclidean`, `supplied`, `provider` or `none`. Euclidean
  feasibility is based on straight-line estimates, not verified transit schedules.
  Unknown opening-hours data must be identified by the caller; the engine only
  sees supplied windows and cannot infer missing calendar data.

Malformed JSON/clock syntax is HTTP 400. Existing body/pair bounds apply. Validation
shares the matrix concurrency pool (429 when exhausted), with a 30-second HTTP
budget and cancellation on timeout/request disposal. A blocking routing operation
keeps its permit until it actually finishes. Provider-backed validation currently
uses the existing bounded matrix builder; supplied and Euclidean matrices avoid
provider queries. Solver, remediation searches and Job storage are not invoked.

## Verification

- End-of-day regression coverage accepts `close_time: 23:59`, rejects unaligned
  ordinary opening/closing times, and verifies an exact latest-start route can
  finish at `23:59`. Manual HTTP calls using actual Trasolve request builders and
  the local troute binary confirmed optimization and detached validation agree
  on a route starting at `23:24` and finishing at `23:59`; a `23:55` closing time
  still returns blocking invalid input. Supplied matrices avoided provider I/O.
- Added four focused tests: collecting multiple pinned-time/window problems,
  equivalence with optimization's schedule semantics, unknown suffix times after
  an unreachable arc, and invalid input blocking before routing.
- The actual local troute binary and Trasolve gateway were started in one network
  namespace. Both endpoints returned the same three issues for a conflicting
  plan, a corrected plan returned an empty issue list, invalid input blocked
  optimization, and the Job collection remained empty after validation.
- Rust formatting, clippy with warnings denied, and the complete Cargo test suite
  are run for this change. The optional SCIP feature and live routing providers
  require their separately configured environments.
