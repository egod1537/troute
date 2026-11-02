# Development

## Code reading order

Start at `src/main.rs` for runtime wiring, then follow `src/http.rs` into
`src/jobs.rs` and `src/service.rs`. Matrix acquisition lives in `src/routing`
and `src/providers/tcache`; optimization starts in `src/solver`, and
`src/schedule.rs` performs the final itinerary validation before `src/storage.rs`
persists Job output. For portfolio scheduling, read
`src/solver/portfolio/descriptor.rs`, `resources.rs`, `objective.rs`,
`telemetry.rs`, and `policy.rs`, then `src/solver/orchestrator/runner.rs`.
See the detailed [request lifecycle](architecture/request-lifecycle.md),
[solver execution](architecture/solver-execution.md), and
[module map](architecture/module-map.md). Implementation coverage and
external prerequisites are tracked in the
[specification status](spec-implementation-status.md).

## Technology

- Rust 1.87 or later
- Axum and Tokio
- Serde
- Docker and Docker Compose
- React, TypeScript, and Vite for the developer testbed

## Getting Started

Install Rust 1.87 or newer using [rustup](https://rustup.rs/). The Docker builder
uses Rust 1.98.1. For container execution, install and start a Docker engine with Docker
Compose support (for example, OrbStack or Docker Desktop on macOS).

Build, test, and run locally:

```sh
cargo build
cargo test
TCACHE_BASE_URL=http://localhost:3200 cargo run
```

The server binds to `0.0.0.0:8080` by default. Check it from another terminal:

```sh
curl -i http://localhost:8080/health
```

Expected: HTTP `200 OK`, JSON `{"status":"ok"}`. Stop the server with Ctrl-C.

```sh
cargo build --release
TCACHE_BASE_URL=http://localhost:3200 TROUTE_PORT=18080 cargo run
```

Then, from another terminal:

```sh
curl -i http://localhost:18080/health
```

`TROUTE_PORT` defaults to `8080` and accepts integers from `1` to `65535`.
Invalid values fail startup with an error on stderr. Startup, bind address,
and shutdown messages go to stdout. The binary does not load `.env` itself;
set variables in the shell for `cargo run`. Set the optional positive integer
`TROUTE_MAX_CONCURRENT_JOBS` limits simultaneously running background jobs and
defaults to `4`. `TROUTE_MAX_ROUTING_PAIRS` defaults to `10000`; routed requests
above that directed-pair count receive `413 ROUTING_PAIR_LIMIT_EXCEEDED` before
provider work starts. Caller-supplied dense or sparse matrices bypass that
provider-query guard. `TROUTE_MAX_OUTSTANDING_JOBS` defaults to `100`; submissions
receive `429 JOB_CAPACITY_EXCEEDED` while that many non-terminal jobs are admitted.
`TROUTE_MAX_CONCURRENT_MATRIX_PREVIEWS` defaults to `2`; excess preview requests
receive `429 MATRIX_PREVIEW_CAPACITY_EXCEEDED` instead of waiting without bound.

troute starts without any Trasolve-specific environment variable. Consumers
configure troute's base URL on their side and poll the Job APIs for progress and
terminal results.

### Real Place ID test fixtures

Tokyo and Seoul 3/5/10-place datasets live in
`tests/fixtures/places`. They are used as offline input by the Rust tests; the
developer testbed exposes only the Seoul fixtures as interactive presets. The
normal test suite never calls Google. Validate fixtures with:

```sh
./scripts/verify_place_fixtures.sh
```

The live tcache/Google check is an ignored test and must be opted into with
`RUN_REAL_ROUTE_TESTS=1`. Collection provenance, generation commands, and the
update policy are documented in `tests/fixtures/places/README.md`.

## Developer testbed

`testbed/` is a compact Blueprint + React + TypeScript + Vite API playground for
developers, separate from the Rust API and the Trasolve user interface. Its
job-centric workspace creates optimize requests from a JSON dialog, shows each
request immediately in the Jobs sidebar, and keeps the structured route result,
raw response, HTTP status, and round-trip latency with the selected job.

On page load, the testbed reads the recent server-side job index, restores each
Job view model, selects the newest job, and fetches its stored Timeline. Browser
state remains a view model; the local troute files are the authoritative
execution record. Reloading the page no longer discards completed history.
Pending and running Job details expose `Job 강제 종료`; confirmation sends the
cancel API request and renders the terminal state as `취소됨` with warning intent.
The Jobs sidebar refreshes recent server history every two seconds while the
page is visible. It merges by `job_id`, preserves the current selection and
browser-only inspection data, and refreshes the selected active Job detail once
per second. The compact refresh button indicates in-flight synchronization and
also supports an immediate manual retry without reloading the page.

The testbed supports Light, Dark, and System themes from the Navbar control.
System is the default and follows browser/OS color-scheme changes. An explicit
selection is stored locally in the browser under `troute.testbed.theme`; theme
selection is frontend-only and does not affect API requests or behavior.

The project icon geometry is canonical in `assets/troute-icon.svg`. The testbed
serves the favicon and Navbar image from `testbed/public/troute-icon.svg`; that
copy uses an explicit Blueprint blue because browser favicons cannot reliably
inherit `currentColor`. Vite validates that the public copy differs from the
canonical SVG only by this explicit color, preventing the two from drifting.
The browser link uses the deterministic `?v=2` suffix to invalidate stale
favicon cache entries. The production image normalizes all Vite output to
world-readable files so the unprivileged Nginx workers can serve public assets.

The Rust API implements `GET /health` and `POST /optimize`. Configure
`VITE_TROUTE_ROUTE_PATH=/optimize` to make the primary action submit the editor
payload; otherwise it continues to run a health check. The client renders
`route` / `total_travel_minutes` from the existing DTOs. Error status and
response bodies remain visible, including malformed JSON and infeasible-route
errors. Route optimization uses a supplied `travel_time_matrix` directly and
only assembles one from tcache pair Route Jobs when the matrix is omitted.

The Seoul and Synthetic testbed presets are self-contained: each ships with a
deterministic demo matrix, so selecting or submitting a preset does not depend
on current routing-provider availability. Seoul presets cover 3-place smoke,
5-place core, 5-place time-window, 5-place intentionally infeasible all-visit
with best-effort fallback, and 10-place day-tour scenarios. Their demo
matrices estimate urban transit time from fixture coordinates; use
`tcache에서 가져오기` to explicitly replace the estimate with live provider
values. Completed Seoul jobs render the input and optimized routes side by side
on coordinate-based maps, alongside the visit timeline; the tabular and raw JSON
results remain available.

For local development (Node 22.12+; Docker builds use Node 24), start the API
with `cargo run` and run these commands in another terminal:

```sh
cd testbed
npm install
npm run dev
```

Open `http://localhost:5173`. The browser uses `VITE_TROUTE_API_BASE_URL=/api`;
Vite forwards `/api/*` to `http://127.0.0.1:8080/*`. For a different API port,
set `TROUTE_DEV_PROXY_TARGET` in `testbed/.env.local`, for example:

```dotenv
TROUTE_DEV_PROXY_TARGET=http://127.0.0.1:18080
```

In Docker, `docker compose up -d --build` builds and starts both `troute` and
`testbed`. The testbed defaults to `http://localhost:8081`; override
`TROUTE_TESTBED_PORT` in the root `.env` when occupied (this Mac uses 18081).
The API's existing host-port override is independent. Both services publish
only on localhost and use `restart: always`.

The testbed image contains built static assets and Nginx, without Node or dev
packages. Nginx serves `/` and proxies `/api/*` to `troute:$TROUTE_PORT` inside
Docker. The browser never resolves a Docker hostname. The same-origin proxy
avoids CORS changes to the Rust API; no wildcard CORS is enabled. A custom
absolute `VITE_TROUTE_API_BASE_URL` requires that API to explicitly allow the
frontend origin; the recommended `/api` setup needs no such configuration.

`VITE_*` values are public build-time configuration. For Compose builds, set
`VITE_TROUTE_API_BASE_URL` and `VITE_TROUTE_ROUTE_PATH` in the root `.env`, then
rebuild. The route path defaults to `/optimize`. For Vite development use
`testbed/.env.local`. Never put
API keys in these variables. Remote private access should forward the testbed
port through your existing private connection; it does not require exposing
the API port or publishing a public domain.

The displayed request latency measures the latest browser round trip, including
the proxy and response transfer. It is not solver execution time. Server-side
Timeline latency is persisted, while this browser-only measurement is available
only for requests made in the current page session.

The main watcher runs in a separate production clone; each new main commit rebuilds and updates
both services. Deployment succeeds only when API health **and** testbed HTTP
checks pass. The testbed also has its own lightweight container healthcheck.

Frontend build and browser regression checks:

```sh
cd testbed
npm ci
npm run build
npx playwright install chromium
npm test
```

Browser tests use explicitly mocked HTTP route fixtures, not a working solver.

## Algorithm validation

The deterministic correctness suite cross-checks the exact solver against an
independent brute-force oracle, compares Pareto pruning on and off, exercises
10-minute slot boundaries, and compares Bit-DP and Blossom perfect matching:

```sh
cargo test --test algorithm_validation
```

For behavior-neutral module migrations, follow
[`docs/refactor-migration.md`](refactor-migration.md) and run the complete
format, lint, regression, contract, and benchmark gate:

```sh
bash scripts/check_refactor_regression.sh
```

Run the release-mode quality and performance matrix with:

```sh
cargo run --release --bin algorithm_validation -- \
  --output-dir benchmark-results \
  --budget-seconds 1,5,30
```

The default matrix uses seeds `42` and `1337` at N=5, 8, 10, 12, 15, 20, 30,
and 50 where each strategy applies. `--n 5,10` and `--seed 42,99` select an
explicit reproducible subset. Quality cases also cover directed time windows,
sparse directed arcs, zero-minute arcs, partial visit, and infeasibility.
Failures include N and seed. The runner writes the legacy reports plus
`solver-quality-results.json`, `manifest.json`, and a Markdown summary containing
proof/bound/gap/thread/source metadata, candidate attempts, first-feasible time,
selected strategy, quality, and timeout classification.

Include SCIP in a feature-enabled run:

```sh
cargo run --release --features solver-mip-scip --bin algorithm_validation -- \
  --output-dir benchmark-results/scip \
  --budget-seconds 1,5,30 \
  --enable-mip
```

Include an operator-supplied LKH executable with
`--lkh-executable /absolute/path/to/LKH`. The checked-in
`benchmark-results/spec-18-smoke` is a small reproducible SCIP smoke report; it
does not claim LKH results.
