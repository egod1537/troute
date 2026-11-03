# HTTP API

`POST /integration/jobs` is the primary asynchronous submission endpoint.
It validates and persists the request, starts background execution, and returns
immediately without waiting for the solver:

```text
HTTP/1.1 202 Accepted
```

```json
{
  "job_id": "route-local-example",
  "status": "pending",
  "created_at": 1789520000000
}
```

Poll `GET /integration/jobs/{job_id}` for `pending -> running -> completed`,
`failed`, or `cancelled`. `POST /optimize` remains as a synchronous legacy
adapter: with persistent storage it submits through the same background Job
runner and waits for the terminal record, so it does not duplicate solver
logic.

Both submission endpoints accept `application/json`. Time values are strict 24-hour
`HH:MM` strings on both request and response; numeric minute values and forms
such as `9:00`, `24:00`, or `09:60` are rejected. Requests must contain 2 to 500
locations. Location `open_time` and `close_time` values must fall on a 10-minute
boundary (`00`, `10`, `20`, `30`, `40`, or `50` minutes), and `stay_minutes`
must be a non-negative multiple of 10. The first location is the fixed start, the last location is the
fixed destination, and only locations between them may be reordered by the
solver. The required `job_id` is an opaque correlation value supplied by the
caller; it must be non-blank and at most 128 characters. Location IDs must be
non-blank strings of at most 512 characters and unique. Place IDs must be
non-blank strings of at most 512 characters when both matrix fields are omitted
and `use_euclidean_matrix` is false. They may be blank when a matrix is supplied
or Euclidean mode is enabled because tcache is bypassed.
Overnight windows are not supported, so `open_time` must not be later than
`close_time`. The request body limit is 1 MiB.

The canonical `OptimizeRouteRequest` wire fields are:

- `job_id`
- `locations[].id`, optional `locations[].name`, `locations[].place_id`,
  `locations[].latitude`, `locations[].longitude`,
  `locations[].open_time`, `locations[].close_time`, and
  `locations[].stay_minutes`; optional `locations[].required` and
  `locations[].priority`
- optional `start_policy`, `start_time`, `travel_mode`, `country_code`,
  `route_provider`, `visit_policy`, `use_euclidean_matrix`, `travel_time_matrix`, and
  `sparse_travel_time_matrix`
- optional `debug.min_job_duration_ms`, `debug.shuffle_result_route`, and
  `debug.shuffle_seed`

Unknown fields are rejected. The compatibility input aliases `countryCode`,
`provider`, and `routeProvider` are accepted, but serialization always uses the
snake-case canonical names above.

When `TROUTE_PORTFOLIO_EXPERIMENT_ENABLED=true`, a request may add the optional
`portfolio_experiment` object with `policy`, `total_budget_ms`,
`logical_cpu_slots`, `seed`, `solver_ids`, and `detailed_trace`. Requested
budget/slots cannot exceed startup limits. With the flag off, the override is
rejected; ordinary requests remain byte-compatible and use the startup policy.

Portfolio implementation boundaries and non-preemptible constraints are in
[`architecture/portfolio-scheduler.md`](architecture/portfolio-scheduler.md).
Reproducible policy experiments are documented in
[`portfolio-benchmark-guide.md`](portfolio-benchmark-guide.md), with
validation scope in
[`portfolio-validation-report.md`](portfolio-validation-report.md).

The optional `travel_mode` field accepts exactly `TRANSIT`, `DRIVING`,
`WALKING`, or `BICYCLING` and defaults to `TRANSIT` when omitted. When troute
builds the matrix, the selected value is forwarded to each tcache Route Job.
When `travel_time_matrix` is supplied directly, routing lookup is bypassed and
`travel_mode` is retained only as request metadata.

`sparse_travel_time_matrix` is the compatible nullable matrix form: a number is
a directed travel time (zero is valid), while `null` means that directed arc is
unreachable. Its diagonal must contain numeric zero. Supply only one matrix
field. Existing numeric `travel_time_matrix` requests are unchanged.

Set `use_euclidean_matrix` to `true` to bypass tcache and build a symmetric
matrix locally from every location's numeric `latitude` and `longitude`.
Coordinates must be supplied as a pair and remain within latitude `[-90, 90]`
and longitude `[-180, 180]`. Straight-line distance is converted to whole
minutes (rounded up) with nominal speeds of 4.8 km/h walking, 15 km/h
bicycling, 30 km/h transit, and 40 km/h driving. This is a cost-saving
estimate: it does not account for roads, barriers, traffic, or transit service.
Do not combine the flag with either caller-supplied matrix field.

`visit_policy` defaults to `ALL_REQUIRED`. The opt-in
`BEST_EFFORT_ON_FAILURE` policy keeps the first and last locations required,
treats an unspecified intermediate `required` as optional, and maximizes the
number of optional visits before the optional `priority` sum (1-100). Explicit
required locations are never removed. Partial successes remain completed Jobs
and include `partial`, `visited_count`, `skipped_locations`, and
`result_quality`; skipped reasons use the conservative
`NOT_SELECTED_DUE_TO_CONSTRAINTS` code.

The optional `country_code` is trimmed, uppercased, and validated as ISO
3166-1 alpha-2. The optional `route_provider` accepts a registered provider name
and is honored only when request overrides are enabled and global force mode is
not active. A global force always wins over a request override.

For manual progress, SSE, and cancellation testing, a request may include the
optional diagnostic setting below:

```json
{
  "debug": {
    "min_job_duration_ms": 4000,
    "shuffle_result_route": true,
    "shuffle_seed": 1234
  }
}
```

`min_job_duration_ms` must be an integer from 0 through 60000. It measures from
Job creation, so time spent pending behind the concurrency semaphore counts
toward the minimum. Real progress stages are exposed near 0% (`accepted`), 20%
(`building_matrix`), 50% (`optimizing_route`), and 80% (`scheduling`); result or error
persistence waits until 100% only when the actual execution finished sooner.
Cancellation interrupts these waits immediately. The option is disabled when
omitted, is retained in `request.json`, applies consistently to successful and
failed terminal states and the legacy `/optimize` adapter.

`shuffle_result_route` defaults to `false`. When enabled, troute leaves the
routing provider and solver untouched, then shuffles only the intermediate
locations in the solver's completed order. The fixed start and destination are
preserved, and the route schedule and total travel time are rebuilt from the
existing travel-time matrix before the result is persisted and returned. Routes
with fewer than two intermediate locations remain unchanged. `shuffle_seed` is
an optional `u64`; equal seeds produce equal debug orders, while omitting it uses
a runtime-random seed. The shuffle and minimum-duration options are independent.

```sh
curl -i -X POST http://127.0.0.1:8080/optimize \
  -H 'Content-Type: application/json' \
  -d '{
    "job_id": "route-local-example",
    "locations": [
      {
        "id": "place-1",
        "place_id": "GOOGLE_PLACE_ID_1",
        "open_time": "00:00",
        "close_time": "23:50",
        "stay_minutes": 0
      },
      {
        "id": "place-2",
        "place_id": "GOOGLE_PLACE_ID_2",
        "open_time": "10:00",
        "close_time": "18:00",
        "stay_minutes": 40
      },
      {
        "id": "place-3",
        "place_id": "GOOGLE_PLACE_ID_3",
        "open_time": "00:00",
        "close_time": "23:50",
        "stay_minutes": 0
      }
    ],
    "start_policy": "LATEST",
    "start_time": "09:00",
    "travel_mode": "DRIVING"
  }'
```

With a matrix assembled from tcache pair queries, the selected route portion of
a successful response looks like the following. The default orchestrator also
adds the `solver_candidates` comparison described below (omitted here for
brevity):

```json
{
  "route": [
    {
      "location_id": "place-1",
      "order": 0,
      "arrival_time": "17:00",
      "service_start_time": "17:00",
      "departure_time": "17:00",
      "wait_minutes": 0,
      "stay_minutes": 0
    },
    {
      "location_id": "place-2",
      "order": 1,
      "arrival_time": "17:15",
      "service_start_time": "17:15",
      "departure_time": "17:55",
      "wait_minutes": 0,
      "stay_minutes": 40
    },
    {
      "location_id": "place-3",
      "order": 2,
      "arrival_time": "18:10",
      "service_start_time": "18:10",
      "departure_time": "18:10",
      "wait_minutes": 0,
      "stay_minutes": 0
    }
  ],
  "total_travel_minutes": 30,
  "start_policy": "LATEST",
  "selected_start_time": "17:00"
}
```

`start_policy` accepts `FIXED`, `EARLIEST`, or `LATEST`. `FIXED` requires
`start_time`; for `EARLIEST` and `LATEST`, an omitted `start_time` defaults to
`00:00`, while a supplied value is the lower bound. For compatibility, omitting
`start_policy` retains the previous `LATEST` behavior.

`OptimizeRouteResponse` always contains `route` and
`total_travel_minutes`. Its current optional metadata fields are
`start_policy`, `selected_start_time`, `solver_candidates`,
`solver_diagnostics`, partial-result fields, `selected_provider`, `provider_selection_reason`,
`provider_selection_source`, `country_code`, and `mode`. Optional metadata is
omitted when it is unavailable.

For `[A, B, C, D]`, `A` and `D` remain fixed while the solver may return an
intermediate order such as `[A, C, B, D]`. The start location's opening window
and stay duration are applied exactly once before the first travel leg.
Intermediate and destination locations retain their time-window and
`stay_minutes` behavior. A
missing `departure_time` is omitted rather than serialized as `null`.

All endpoint failures use a JSON envelope rather than an HTML error page:

```json
{
  "error": {
    "code": "NO_FEASIBLE_ROUTE",
    "message": "No feasible route was found.",
    "detail": "no feasible route",
    "failure_detail": {
      "type": "TIME_WINDOW_VIOLATION",
      "location_id": "museum",
      "arrival_time": "10:00",
      "service_start_time": "10:00",
      "required_departure": "11:00",
      "close_time": "10:30",
      "stay_minutes": 60
    },
    "suggestions": [
      {
        "type": "REDUCE_STAY_TIME",
        "reason": "This is the maximum stay that lets the evaluated route finish service by closing time.",
        "confidence": "exact",
        "location_id": "museum",
        "current_value": 60,
        "suggested_value": 30,
        "current_minutes": 60,
        "suggested_max_minutes": 30
      }
    ]
  }
}
```

`failure_detail` describes the computed failure while the optional
`suggestions` array contains independently actionable changes. Suggestion
`type` is a stable enum suitable for UI action mapping; `confidence` is
`exact` when the value follows from a necessary constraint and `heuristic`
otherwise. Suggestions are omitted when the server has no calculation-backed
recommendation.

After an optimization failure, troute may spend up to
`TROUTE_REMEDIATION_BUDGET_MS` on validation probes. A
`CHANGE_TRAVEL_MODE` suggestion is emitted only when a real routing matrix
query succeeds for that mode. `REMOVE_LOCATION` is emitted only when the
reduced problem is feasible; small probes use exact bit-DP and larger probes
use a deterministic greedy feasibility check. Probe count is capped by
`TROUTE_REMEDIATION_MAX_REMOVE_PROBES`. `SPLIT_DAY` can be derived from a
successful removal probe by moving the identified location to another day.

Stable suggestion types are `REDUCE_STAY_TIME`, `MOVE_LOCATION_EARLIER`,
`MOVE_LOCATION_LATER`, `START_EARLIER`, `START_LATER`,
`CHANGE_START_POLICY`, `CHANGE_START_LOCATION`, `CHANGE_END_LOCATION`,
`REMOVE_LOCATION`, `CHANGE_TRAVEL_MODE`, `SPLIT_DAY`, and `RETRY_ROUTING`.

Suggestions are ranked deterministically before serialization. The default
order favors start-time changes, stay reduction, location reordering, policy
or travel-mode changes, endpoint changes, removal, and finally splitting the
day. Within the same action class, verified/exact candidates and smaller
minute changes rank first; routing work and unverified recovery add penalties.
Equivalent actions and conflicting earlier/later actions are collapsed before
the top `TROUTE_MAX_FAILURE_SUGGESTIONS` entries are returned.

Malformed or invalid requests return 400, a non-JSON content type returns 415,
and an oversized body returns 413. An infeasible route or schedule returns 422
with `NO_FEASIBLE_ROUTE`. A routing-provider outage returns 503. Unexpected
solver or schedule failures return 500. No permissive browser CORS is enabled;
the supported integration is server-to-server.

## Solver and routing architecture

The default server uses `SolverOrchestrator` with one global deadline. It first
tries `ExactBitDpSolver` when applicable and returns immediately when exact
finishes with a feasible optimum. Otherwise it builds clustered, Greedy, MST
Double-Tree, and Christofides baselines, then spends the remaining budget on
repeated time-bounded SA chunks. The first SA round uses Greedy, MST,
Christofides, and clustered initial routes; later rounds interleave the current
global best as a warm start with deterministic restarts. A bounded worker queue
enforces `SOLVER_CPU_SLOTS` for baselines, with `SOLVER_MAX_CONCURRENCY` as a
legacy fallback. The global deadline and `SOLVER_STRATEGY_TIMEOUT_MS`
cooperatively cancel overlong work.

Every route is converted to the same `SolverCandidate` and evaluated by the
shared `ObjectiveEvaluator`. `CandidateSelector` ranks feasible routes first,
then applies the requested start policy (`FIXED`, earliest start, or latest
start), followed by earliest finish, least directed travel, and least waiting.
Registration order is the deterministic final tie-break. If no
feasible candidate remains, the orchestrator returns `NoFeasibleRoute`.
Successful API responses include `solver_candidates` with the selected `Best`
candidate, every individual strategy result, elapsed time, objective fields,
state/iteration counts, seed, timeout flag, and error. The Testbed renders the
same comparison table rather than hiding losing or failed strategies. Optional
`solver_diagnostics` reports the total/baseline/SA timing, SA run count, global
best updates, and termination reason.

The exact solver supports at most 15 locations, keeps the first and last
request locations as fixed endpoints, and reorders every intermediate
location with bitmask DP. Solver state and final scheduling use exact minutes;
the input window/stay contract remains ten-minute aligned. Each
`(visited_mask, last_location)` cell contains a Pareto frontier over time and
cost; objective scoring and frontier dominance are replaceable policies.

`ClusteredSolver` divides intermediate locations with the replaceable
`ClusterStrategy`, orders clusters with `ClusterOrderStrategy`, and solves each
bounded subproblem by reusing `ExactBitDpSolver`. Its exit proxy includes the
directed cost toward the next cluster when choosing an internal route. The
merged route is validated end-to-end, then `BoundarySwapLocalImprovement`
tries cross-cluster boundary swaps and accepts only improvements according to
the same objective evaluator. The default travel-time clustering and greedy
cluster ordering can be replaced without changing solver orchestration.

`SimulatedAnnealingSolver` is the alternative large-N heuristic. Its default
initial-route strategy tries `ClusteredSolver`, MST Double-Tree, directed
greedy ordering, then a random permutation. Each iteration randomly applies
swap, relocate, or 2-opt. Feasible routes use the shared objective evaluator;
infeasible routes remain searchable through configurable lateness,
day-overflow, and required-location penalties. SA separately retains the best
feasible route and verifies it with the normal evaluator before returning.
Temperature, geometric cooling, minimum temperature, iteration/time limits,
optional random seed, and penalty weights are all represented by
`SimulatedAnnealingConfig`; no seed is forced by default.

`MstDoubleTreeInitialRoute` is a deterministic, fast baseline and reusable
`InitialRouteGenerator`. It symmetrizes the directed matrix with the
replaceable `SymmetricDistanceStrategy` (average by default, with min and max
strategies also available), builds a deterministic Prim MST, walks every tree
edge twice, and shortcuts repeated vertices. The final destination is moved to
the fixed endpoint position required by troute. `GreedyInitialRoute` and
`ClusteredInitialRoute` implement the same generator interface, and
`MstClusterOrderStrategy` makes the same approach available as a replaceable
cluster-order policy.

Symmetric MST weights are never used as the final route score. Generated routes
are evaluated with the original directed `TravelTimeMatrix`, time windows, and
stay durations. An infeasible generated route may seed SA's penalty search, but
it is not a valid standalone result. The familiar Double-Tree 2-approximation
guarantee applies only to ordinary symmetric metric TSP instances satisfying
the triangle inequality. It is **not** claimed for troute's directed,
fixed-endpoint, time-window, and stay-time problem.

`ChristofidesInitialRoute` reuses the same symmetric-distance and MST boundary,
then extracts the MST's odd-degree vertices and injects a
`PerfectMatchingStrategy`. `BitDpPerfectMatching` computes exact MWPM with
`dp[matched_mask]` up to 20 odd vertices. `BlossomPerfectMatching` adapts the
exact, deterministic `integer-blossom` solver for larger sets. The default
`AutoPerfectMatching` chooses bit-DP at or below the configurable threshold
(20 by default) and Blossom above it. `MatchingStrategyConfig` can force either
strategy; a forced strategy's failure is returned explicitly and never causes
a silent greedy fallback.

The Blossom dependency is pinned to `integer-blossom` 1.2.1. It exposes
minimum-weight perfect matching for complete general graphs with integer
weights, uses an O(V³) primal-dual Blossom implementation, has no transitive
dependencies, and is MIT licensed. Its upstream tests compare against a
brute-force oracle and metamorphic properties. The crate was initially released
in July 2026 and follows semantic versioning, but remains a young dependency;
pinning and the Bit-DP cross-check tests keep upgrades explicit. Its Rust 1.87
MSRV raises troute's declared MSRV to 1.87.

After matching, Christofides combines the matching with the MST, runs a
deterministic Eulerian multigraph traversal, and shortcuts repeated vertices.
Like the Double-Tree generator, the resulting route is evaluated only with the
original directed matrix and shared schedule/objective evaluator; it can also
be supplied directly as the SA initializer.

Christofides' 1.5-approximation guarantee applies only to the ordinary
symmetric metric TSP with triangle inequality. No 1.5 guarantee is claimed for
troute's directed, fixed-endpoint, time-window, or stay-time model.

Routing and optimization have a strict matrix boundary:

```text
OptimizeRouteRequest
  -> OptimizationProblem
  -> RoutingProvider + RoutingContext
  -> TravelTimeMatrix
  -> SolverOrchestrator
       -> ExactBitDpSolver (small N)
       -> ClusteredSolver -> ExactBitDpSolver per cluster
       -> MST Double-Tree / Christofides candidates
       -> SimulatedAnnealingSolver multi-start
            -> Clustered / Christofides / MST Double-Tree / Greedy seeds
       -> ObjectiveEvaluator -> CandidateSelector -> Best Route
  -> Schedule
  -> OptimizeRouteResponse
```

The solver only reads `OptimizationProblem` and `TravelTimeMatrix`; it does not
call HTTP, tcache, caches, Google APIs, or place-ID lookup code.
`DevelopmentRoutingProvider` and `StaticMatrixRoutingProvider` remain test
utilities. The server runtime wires `TcacheTravelTimeProvider` through
`PairwiseMatrixRoutingProvider`. The former knows only tcache's existing Route
Job HTTP contract and the latter builds the complete directed matrix. The
solver remains isolated from pair-query, polling, cache, and HTTP details.

`DevelopmentRouteSolver` remains available for tests and development flows
which intentionally preserve input order, but it is no longer the default
server solver.

Compare exact and clustered quality and work counters on the same small
problem with:

```sh
cargo bench --bench clustered_solver
```

The benchmark compares Bit-DP and Blossom MWPM cost and runtime on the same
small odd set, also runs Blossom on a large odd set, and reports strategy-owned
memory estimates (the Blossom figure is its flattened input buffer and excludes
the crate's reused O(n²) thread-local pool). It then compares greedy, MST
Double-Tree, bit-DP/Blossom/auto Christofides, clustered, exact, and simulated
annealing. Initial score and feasibility, elapsed time, score after SA,
objective gaps, cluster/state/frontier counts, and move counters are printed.
It also prints every orchestrator candidate, the `Best` marker, seed, failure,
and total wall-clock time for both small and large inputs.
Set `MATCHING_STRATEGY` to force the configured Christofides benchmark row.

## One-way Job API integration

The supported dependency direction is `Trasolve -> troute` only:

```text
Trasolve
  ├─ POST /integration/jobs
  ├─ POST /optimize (legacy synchronous adapter)
  ├─ GET /integration/jobs?limit=50
  ├─ GET /integration/jobs/{job_id}
  ├─ GET /integration/jobs/{job_id}/events
  ├─ GET /integration/jobs/{job_id}/timeline
  └─ POST /integration/jobs/{job_id}/cancel
        ↓
      troute
```

troute validates and persists the request before spawning a Tokio task. The
blocking optimization pipeline runs on Tokio's blocking pool, records progress
through `accepted`, `building_matrix`, `optimizing_route`, and `scheduling`, then stores
either the result or error before its terminal state. The Job Store is
authoritative; the lifetime of the submitting HTTP request is independent of
optimization. It performs no HTTP request to Trasolve. A consumer can poll the
Job detail endpoint to read status, progress, result, error, and cancellation
state:

```json
{
  "job_id": "route-example",
  "status": "running",
  "stage": "optimizing_route",
  "progress": 60,
  "last_message": "Solving route.",
  "created_at": 1789520000000,
  "updated_at": 1789520005000,
  "completed_at": null,
  "request": {},
  "result": null,
  "error": null
}
```

## Job progress streaming

An active Job can also be followed with Server-Sent Events:

```sh
curl -N http://127.0.0.1:8080/integration/jobs/JOB_ID/events
```

The stream sends one persisted `snapshot` immediately, followed by `progress`
events and one `completed`, `failed`, or `cancelled` terminal event. Every Job
event has a monotonically increasing `id`; the stream closes after terminal
delivery. Idle connections receive a `: heartbeat` comment every 15 seconds.
Responses use `text/event-stream`, `Cache-Control: no-cache`,
`Connection: keep-alive`, and `X-Accel-Buffering: no`.

Every event uses the same public envelope, independently of the full Job detail
and its persistence representation:

```text
event: progress
id: 17
data: {"sequence":17,"updated_at":123456789,"state":{"job_id":"route-example","status":"running","stage":"fetching_travel_times","progress":33,"last_message":"Fetched 3 / 10 travel-time pairs.","error":null,"result":null}}
```

`id` is always the decimal form of `data.sequence`. `sequence` is the Job's
logical SSE revision and is persisted separately from the detail DTO, so a
terminal reconnect keeps the same current revision. `updated_at` is copied from
the persisted Job state and is not the transmission time. Consumers must read
Job fields from `data.state`; `job_id`, `status`, request data, `created_at`,
and `completed_at` are never flat envelope fields. `stage`, `last_message`,
`error`, and `result` are always present and use JSON `null` when absent.

`snapshot` permits every Job status. `progress` carries only `pending` or
`running` state and a stage. `completed` has a non-null result and null error;
`failed` has a non-null error and null result; `cancelled` carries cancelled
state with null result and error. A failed error retains `code`, `message`,
`detail`, `failure_detail`, and `suggestions`. The latter two are stable,
machine-readable public contracts.

Public progress stages, in order, are `accepted`, `validating_request`,
`selecting_provider`, `preparing_matrix`, `fetching_travel_times`,
`building_matrix`, `generating_candidates`, `optimizing_route`,
`selecting_best_candidate`, `scheduling`, `validating_schedule`, and
`finalizing_result`. The legacy input value `solving` is accepted when reading
stored data, but new payloads serialize only `optimizing_route`.

During rollout, Trasolve may accept the former flat SSE payload as a temporary
read-only compatibility fallback. troute emits only the canonical envelope;
it does not maintain two output formats. Inventory any additional consumers
before deployment and remove the fallback after all producers are upgraded.

The broadcast channel is only a low-latency notification path. State is always
persisted before publication, and the Job Store remains authoritative. A new or
reconnected client receives the latest snapshot; clients should use
`GET /integration/jobs/{job_id}` if an SSE connection is interrupted. Slow
subscribers never block workers and recover from broadcast lag using another
persisted snapshot. That recovery snapshot uses the current sequence at the
stable snapshot read, and its SSE `id` matches the envelope sequence. Opening a
stream for an already-terminal Job yields one snapshot containing its result or
error and then closes.

The testbed uses SSE for the selected active Job while retaining polling for
Job-list discovery. Its nginx proxy disables response buffering and uses a
one-hour upstream read timeout; the edge Caddy route flushes event chunks
immediately.

This supports a local Trasolve calling local troute, local Trasolve calling the
deployed troute, and deployed Trasolve calling deployed troute. Configure only
the caller, for example:

```sh
# Host-native Trasolve -> Compose troute
TROUTE_BASE_URL=http://127.0.0.1:18080 npm run dev -w @trasolve/backend

# Local Trasolve -> deployed troute
TROUTE_BASE_URL=https://troute.mangagaki.net npm run dev -w @trasolve/backend
```

When both services share a Docker network, use `http://troute:8080` instead.
For direct `cargo run`, troute defaults to `http://127.0.0.1:8080` unless
`TROUTE_PORT` is overridden. No reciprocal base URL is configured in troute.

## Job cancellation

An active persisted job can be cancelled with an empty POST request:

```text
POST /integration/jobs/{job_id}/cancel
```

Only `pending` and `running` jobs are cancellable. The endpoint returns the
terminal `cancelled` state and signals the service and solver cancellation
token. Cancellation is checked at every major pipeline boundary and is
available to solver implementations through `SolverInput`. The synchronous
`/optimize` request returns HTTP 409 with `JOB_CANCELLED` after it stops.

Terminal transition is guarded by the file store lock: whichever of result,
failure, or cancellation is persisted first wins. A cancellation winner keeps
the last progress value, sets `completed_at`, writes no `error.json` or
`result.json`. Cancelling a completed, failed, or already cancelled job returns
`JOB_NOT_CANCELLABLE`; an unknown job returns `JOB_NOT_FOUND`.

For local end-to-end verification, submit an optimization through Trasolve and
then poll the same Job directly from troute:

```sh
curl -i -X POST http://127.0.0.1:43127/api/troute/optimize \
  -H 'Content-Type: application/json' \
  --data @optimize-request.json

curl -i http://127.0.0.1:18080/integration/jobs/JOB_ID
```

After success, the job is `completed` with progress 100 and a populated result.
troute owns this local execution record; the consumer only reads it.

## Local job records and observation timeline

troute stores validated optimize requests and their execution state as readable
UTF-8 JSON under `TROUTE_DATA_DIR`. The default is `.local/troute`; Docker uses
`/data`. Each job has its own directory:

```text
.local/troute/
├── index.json
└── jobs/<encoded-job-id>/
    ├── request.json
    ├── state.json
    ├── result.json       # successful jobs only
    ├── error.json        # failed jobs only
    ├── timeline.jsonl
    └── solver-telemetry.jsonl # bounded, append-only solver events when collected
```

`state.json`, result/error files, and `index.json` are replaced atomically via a
temporary file and rename. `timeline.jsonl` is append-only with one complete
`JobTimelineEntry` JSON value per line. Job IDs are safely encoded only for the
directory name; the original ID remains unchanged in stored JSON and APIs.

An existing job directory is never overwritten. Reusing a `job_id` returns HTTP
409 with `DUPLICATE_JOB_ID`. At startup, completed, failed, and cancelled jobs
are retained; pending or running jobs are marked failed with
`JOB_INTERRUPTED`. There is no automatic retention deletion in this version.

Read recent jobs, one stored job, or its timeline with:

```text
GET /integration/jobs?limit=50
POST /integration/jobs
GET /integration/jobs/{job_id}
GET /integration/jobs/{job_id}/events
POST /integration/jobs/{job_id}/cancel
GET /integration/jobs/{job_id}/timeline
```

An unknown timeline returns HTTP 200 with an empty `entries` list; an unknown
job detail returns 404. Timeline entries are sorted by millisecond timestamp,
preserving file insertion order when timestamps match. Each real HTTP exchange
has separate `REQUEST` and `RESPONSE` entries sharing a `pair_id`. The timeline
contains traffic observed by troute, including optimize and cancellation HTTP
exchanges and successful Job detail inspection requests. It contains no
fabricated browser traffic or removed Trasolve callback entries. Unknown Job
lookups are not recorded, so observation cannot create a phantom Job directory.

Only `Content-Type`, `Accept`, and `User-Agent` headers are eligible for
recording. Authorization, cookies, API keys, and other headers are never stored.
