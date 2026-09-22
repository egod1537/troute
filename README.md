# troute

Routing and route optimization engine for Trasolve.

## Overview

troute is an independent server intended to handle routing and itinerary optimization computations for Trasolve. It separates computation-heavy, algorithm-focused work from the Trasolve frontend and general service logic.

The project is expected to address problems such as:

- Computing routes between multiple places
- Optimizing visit order
- Evaluating routes by distance and travel time
- Optimizing itineraries with time constraints

These capabilities are planned and are not yet implemented.

## Goals

- Generate distance matrices
- Optimize routes and visit order
- Explore TSP- and VRP-family problems
- Support time-window constraints
- Account for opening hours and stay duration
- Support user-defined constraints
- Use travel-time data from real-world mapping services

The long-term goal is not only to provide shortest-path calculations, but to develop an optimization engine that can account for the constraints involved in real travel itineraries.

## Architecture

The current high-level structure is:

```text
Browser / React
   |
   v
Trasolve Backend
   |
   | HTTP / API
   v
troute
   |
   +-- HTTP health endpoint
   +-- HTTP optimize endpoint
   +-- API types
   +-- Routing Provider (development or tcache adapter)
   +-- Travel Time Matrix
   +-- Route Solver
   +-- Schedule Calculation
```

The component interfaces and v0 data flow are defined. The HTTP server exposes
`GET /health`, asynchronous `POST /integration/jobs`, the legacy synchronous
`POST /optimize`, and polling APIs under `/integration/jobs`. Background jobs
execute the provider -> solver -> schedule service pipeline and persist
progress, result, error, cancellation, and observation data locally. The
currently wired provider and solver are
deterministic development provider or a tcache-backed Travel Time Matrix
provider. Provider-specific route lookup, caching, and Google credentials stay
inside tcache; the solver and scheduling layers only receive a matrix.
Trasolve calls troute in one direction only. troute neither requires a Trasolve
address nor sends callbacks to it. The separate developer testbed uses its own
same-origin API proxy.

## Tech Stack

- Rust
- axum / tokio
- Serde
- Docker / Docker Compose
- Developer testbed: React / TypeScript / Vite

## Development Roadmap

- [x] Basic Rust project setup
- [x] v0 request, response, location, and route data models
- [x] Travel time matrix model
- [x] Routing provider and solver interfaces
- [x] Schedule calculation for a supplied visit order
- [x] Runnable HTTP server and Docker Compose environment
- [x] `POST /optimize` HTTP integration contract
- [x] Asynchronous Job submission, polling, and cancellation
- [x] Developer testbed with health checks, input editor, and request timing
- [x] tcache Travel Time Matrix integration
- [ ] Simple greedy route solver
- [ ] 2-opt or similar local optimization
- [ ] External routing data integration
- [ ] Constraint-aware route optimization

## Getting Started

Install Rust 1.80 or newer using [rustup](https://rustup.rs/). The Docker builder
uses Rust 1.98.1. For container execution, install and start a Docker engine with Docker
Compose support (for example, OrbStack or Docker Desktop on macOS).

Build, test, and run locally:

```sh
cargo build
cargo test
cargo run
```

The server binds to `0.0.0.0:8080` by default. Check it from another terminal:

```sh
curl -i http://localhost:8080/health
```

Expected: HTTP `200 OK`, JSON `{"status":"ok"}`. Stop the server with Ctrl-C.

```sh
cargo build --release
TROUTE_PORT=18080 cargo run
```

Then, from another terminal:

```sh
curl -i http://localhost:18080/health
```

`TROUTE_PORT` defaults to `8080` and accepts integers from `1` to `65535`.
Invalid values fail startup with an error on stderr. Startup, bind address,
and shutdown messages go to stdout. The binary does not load `.env` itself;
set variables in the shell for `cargo run`. Set the optional positive integer
`TROUTE_MAX_CONCURRENT_JOBS` to limit simultaneously running background jobs;
when it is unset, submitted jobs run without a semaphore limit.

troute starts without any Trasolve-specific environment variable. Consumers
configure troute's base URL on their side and poll the Job APIs for progress and
terminal results.

## Routing provider configuration

`ROUTING_PROVIDER=development` is the default and retains the deterministic
15-minute development matrix. Set `ROUTING_PROVIDER=tcache` to create one
matrix Job in tcache for each optimization request. `TCACHE_BASE_URL` is then
required; invalid configuration fails application startup.

| Variable | Default | Description |
| --- | --- | --- |
| `ROUTING_PROVIDER` | `development` | `development` or `tcache` |
| `TCACHE_BASE_URL` | none | tcache server base URL; required for `tcache` |
| `TCACHE_MATRIX_POLL_INTERVAL_MS` | `250` | positive status polling interval |
| `TCACHE_MATRIX_TIMEOUT_MS` | `30000` | positive total matrix request timeout |

The adapter creates `POST /api/route/matrix/jobs`, polls the returned Job,
fetches `durationSeconds`, validates the location order and matrix shape, and
converts seconds to whole minutes by rounding up. It attempts the tcache cancel
endpoint when its total timeout expires. The current troute v0 domain contains
only a wall-clock `start_time`, not a calendar date or timezone, while the
existing `RoutingProvider` boundary accepts locations only. Consequently the
adapter sends the current UTC instant as matrix `departureTime`; adding a
dated optimization request can refine this later without exposing tcache HTTP
details to the solver.

### Real Place ID test fixtures

Tokyo and Seoul 3/5/10-place datasets live in
`tests/fixtures/places`. They are used as offline input by the Rust tests and
generate the Place ID presets shown by the tcache Matrix Testbed. The normal
test suite never calls Google. Validate fixtures with:

```sh
./scripts/verify_place_fixtures.sh
```

The live tcache/Google check is an ignored test and must be opted into with
`RUN_REAL_ROUTE_TESTS=1`. Collection provenance, generation commands, and the
update policy are documented in `tests/fixtures/places/README.md`.

## HTTP API

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
locations. The first location is the fixed start, the last location is the
fixed destination, and only locations between them may be reordered by the
solver. The required `job_id` is an opaque correlation value supplied by
Trasolve; it must be non-blank and at most 128 characters. Location IDs and
Place IDs must be non-blank strings of at most 512 characters, and location IDs
must be unique. Overnight windows are not supported, so `open_time` must not be
later than `close_time`. The request body limit is 1 MiB.

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
(`building_matrix`), 50% (`solving`), and 80% (`scheduling`); result or error
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
        "close_time": "23:59",
        "stay_minutes": 0
      },
      {
        "id": "place-2",
        "place_id": "GOOGLE_PLACE_ID_2",
        "open_time": "10:00",
        "close_time": "18:00",
        "stay_minutes": 45
      },
      {
        "id": "place-3",
        "place_id": "GOOGLE_PLACE_ID_3",
        "open_time": "00:00",
        "close_time": "23:59",
        "stay_minutes": 0
      }
    ],
    "start_time": "09:00"
  }'
```

The current deterministic development implementation returns:

```json
{
  "route": [
    {
      "location_id": "place-1",
      "order": 0,
      "arrival_time": "09:00",
      "departure_time": "09:00"
    },
    {
      "location_id": "place-2",
      "order": 1,
      "arrival_time": "09:15",
      "departure_time": "10:45"
    },
    {
      "location_id": "place-3",
      "order": 2,
      "arrival_time": "11:00",
      "departure_time": "11:00"
    }
  ],
  "total_travel_minutes": 30
}
```

For `[A, B, C, D]`, `A` and `D` remain fixed while the solver may return an
intermediate order such as `[A, C, B, D]`. The start location's stay duration
and opening window are ignored for the initial departure. Intermediate and
destination locations retain their time-window and `stay_minutes` behavior. A
missing `departure_time` is omitted rather than serialized as `null`.

All endpoint failures use a JSON envelope rather than an HTML error page:

```json
{
  "error": {
    "code": "INVALID_REQUEST",
    "message": "The optimize request is invalid."
  }
}
```

Malformed or invalid requests return 400, a non-JSON content type returns 415,
and an oversized body returns 413. An infeasible route or schedule returns 422
with `NO_FEASIBLE_ROUTE`. A routing-provider outage returns 503. Unexpected
solver or schedule failures return 500. No permissive browser CORS is enabled;
the supported integration is server-to-server.

### Current provider and solver status

`DevelopmentRoutingProvider` supplies zero minutes on the matrix diagonal and
a fixed 15 minutes between every pair of different locations.
`DevelopmentRouteSolver` preserves the input order, including the fixed first
and last locations. This behavior is deterministic and exercises the real
`RouteOptimizationService` and schedule, but it is **not route optimization and
does not use Google travel data**. The implementations are isolated in
`src/development.rs` so they can be replaced without changing the HTTP handler
or wire contract. Schedule infeasibility is returned as an error; the
placeholders do not alter request semantics or invent a successful route.

### One-way Job API integration

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
through `accepted`, `building_matrix`, `solving`, and `scheduling`, then stores
either the result or error before its terminal state. The Job Store is
authoritative; the lifetime of the submitting HTTP request is independent of
optimization. It performs no HTTP request to Trasolve. A consumer can poll the
Job detail endpoint to read status, progress, result, error, and cancellation
state:

```json
{
  "job_id": "route-example",
  "status": "running",
  "stage": "solving",
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

### Job progress streaming

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

The broadcast channel is only a low-latency notification path. State is always
persisted before publication, and the Job Store remains authoritative. A new or
reconnected client receives the latest snapshot; clients should use
`GET /integration/jobs/{job_id}` if an SSE connection is interrupted. Slow
subscribers never block workers and recover from broadcast lag using another
persisted snapshot.

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

### Job cancellation

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

### Local job records and observation timeline

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
    └── timeline.jsonl
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

## Docker

The multi-stage Dockerfile builds the binary and copies it into a Debian slim
runtime image. It runs as a non-root user without the Rust toolchain.

```sh
docker compose build
docker compose up -d
curl -i http://127.0.0.1:18080/health
docker compose logs -f troute
docker compose down
```

To build and run in the foreground, use `docker compose up --build`. To rebuild
and run in the background, use `docker compose up -d --build`.

Compose publishes the host port on `127.0.0.1` only. The application still binds
to `0.0.0.0` inside the container, so containers on its Docker network can reach
it. The application/container port defaults to `8080`; the host port defaults
to `18080` because SFTPGo uses host port `8080` on the deployment Mac.

Compose sets `TROUTE_DATA_DIR=/data` and bind-mounts
`./runtime/troute:/data`. Both
`.local/` and `runtime/` are ignored by Git. `docker compose build`, `up`, and
the deployment script do not remove the host runtime directory, so records
survive image and container replacement. Inside the troute container,
`127.0.0.1` refers to that container. Consumers on the same Docker network use
`http://troute:8080`; host-native consumers use the published loopback port.
troute has no outbound Trasolve configuration.

To make the host and container ports explicit, create a local configuration:

```sh
cp .env.example .env
```

The example already sets `TROUTE_HOST_PORT=18080` and `TROUTE_PORT=8080`. Run
`docker compose up -d --build` and check `http://127.0.0.1:18080/health`.
`TROUTE_HOST_PORT` changes only the published host port; `TROUTE_PORT` sets the
application/container port. Compose automatically reads `.env` and keeps the
port mapping consistent. Do not use `127.0.0.1:18080` from another container;
the testbed uses the Docker-network address `troute:8080`.

No API container healthcheck is configured: the slim runtime has no HTTP client,
and adding one solely for this check is unnecessary at this stage. Verify
`/health` from the host or the Trasolve backend. The restart policy restarts an
exited container; it does not detect an unresponsive process.

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
errors. Results currently come from the documented deterministic development
provider and solver, not a production optimization algorithm.

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
only on localhost and use `restart: unless-stopped`.

The testbed image contains built static assets and Nginx, without Node or dev
packages. Nginx serves `/` and proxies `/api/*` to `troute:$TROUTE_PORT` inside
Docker. The browser never resolves a Docker hostname. The same-origin proxy
avoids CORS changes to the Rust API; no wildcard CORS is enabled. A custom
absolute `VITE_TROUTE_API_BASE_URL` requires that API to explicitly allow the
frontend origin; the recommended `/api` setup needs no such configuration.

`VITE_*` values are public build-time configuration. For Compose builds, set
`VITE_TROUTE_API_BASE_URL` and optional `VITE_TROUTE_ROUTE_PATH` in the root
`.env`, then rebuild. For Vite development use `testbed/.env.local`. Never put
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

## Mac mini operation

Run `docker compose up -d --build` from the `main` checkout. The service uses
`restart: unless-stopped`, so it can restart when the Docker engine starts again.
Configure the installed Docker engine to start automatically after reboot/login,
and keep the Mac awake for continuous service. Compose does not start the Docker
engine or wake the Mac. Reboot behavior depends on that host configuration.
An explicitly stopped container remains stopped; use `docker compose up -d` to
start it again. `docker compose down` removes it.

## Automatic deployment

The Mac mini polls **origin/main every minute** using cron, following the jsb1
pull-based deployment pattern. Production deployment does not use GitHub
Actions or inbound SSH. The former `TROUTE_DEPLOY_*` Actions Secrets are no
longer used. The Mac uses its existing GitHub credentials for `ls-remote`/fetch;
GitHub must be reachable non-interactively from the cron user's session.

Use a **dedicated production clone on main**, separate from development. Update
that clone to the published main and run there:

```sh
./auto-deploy.sh --once
./auto-deploy.sh --install
./auto-deploy.sh --status
```

`--once` checks the remote SHA and deploys if needed. On the first run there is
no recorded deployment, so it builds and verifies main even if HEAD already
matches origin. `--install` installs one `# troute-auto-deploy` cron entry at
`* * * * *`; reinstalling replaces that entry and preserves other cron jobs.
It requires a clean, published main checkout. The next cron tick performs the
first check if `--once` was skipped. To remove only the watcher:

```sh
./auto-deploy.sh --uninstall
```

The watcher compares remote and successfully deployed SHAs, then calls
`scripts/deploy.sh`. The deploy script fetches main, checks out/reset tracked
files to origin/main, builds both services with `docker compose build`, and runs
`docker compose up -d --no-build`. Build failure leaves existing containers
in place. If main advances between polling and fetch, deployment is deferred
to the next poll so the recorded SHA always matches the code being built.

Success requires the API `/health` to return HTTP 200 and `{"status":"ok"}`,
the testbed `/` to return HTTP 200, and `/troute-icon.svg` to return a non-empty
HTTP 200 response whose Content-Type starts with `image/svg+xml`. Both published
host ports are discovered from their containers, including `.env` overrides. A
testbed or favicon failure marks the deployment failed without stopping the API.
Health checks retry up to 20 times at 3-second intervals with a 5-second request
timeout. Only success updates `deployed-commit`. The same failed commit waits
300 seconds after failure before retrying; a newer commit bypasses that delay.

The deploy script also reports the exact checked-out commit through GitHub's
Commit Status API under the stable `deploy/troute` context. It publishes
`pending` before the image build, `success` only after all health checks pass,
and `failure` when build, startup, health checks, or an interrupt fails the
deployment. The check links to `https://troute.mangagaki.net`. Status reporting
is best-effort: a missing token or GitHub API outage is logged but never changes
the deployment result or its original exit code.

Set `GITHUB_TOKEN` only in the Mac mini deployment user's private runtime
environment. For a fine-grained PAT, grant repository access only to
`egod1537/troute` and **Commit statuses: Read and write**. No administration,
Actions, or contents-write permission is required. For the installed cron job,
an environment assignment in that user's private crontab is sufficient; the
installer preserves unrelated crontab lines:

```text
GITHUB_TOKEN=github_pat_REPLACE_ON_THE_DEPLOYMENT_HOST
```

Do not add this value to the repository, `.env.example`, Compose configuration,
or frontend build arguments. `crontab -l` will reveal a crontab assignment to
that local user, so protect the deployment account accordingly. If
`GITHUB_TOKEN` is absent, the script logs
`[deploy] GitHub status reporting: disabled (GITHUB_TOKEN missing)` and deploys
normally. When configured it logs only that reporting is enabled, never the
token value. API failures include the attempted state and seven-character SHA
for diagnosis without exposing the authorization header.

To change the delay for future cron runs, reinstall with, for example:

```sh
TROUTE_AUTO_DEPLOY_RETRY_SEC=600 ./auto-deploy.sh --install
```

Status, last attempted SHA, successful SHA, and retry time live under
`.git/troute-deploy/` (the Git directory for linked worktrees), outside tracked
files. Logs include detected SHA, build/start/health output, and final result:

```sh
tail -f "$(git rev-parse --absolute-git-dir)/troute-deploy/logs/auto-deploy.log"
```

Watcher and deployment locks prevent overlapping polls and builds. If an
unclean shutdown leaves a lock behind, `--status` reports it; confirm no watcher
or deployment process is running before removing the stale lock. Uninstalling
preserves state/logs and leaves the container running.

**Deployment discards tracked local edits.** Runtime secrets stay in ignored
`.env`; scripts neither overwrite it nor run `git clean`. A tracked runtime
`.env` file is rejected before reset. When moving an existing service to a new
clone, preserve `TROUTE_HOST_PORT` and set `COMPOSE_PROJECT_NAME` in `.env` to
its existing Compose project name so the service is updated in place.

Git, Bash, curl, cron, and a running Docker engine with Compose are required.
The script supplies common macOS executable paths. Keep the Mac awake and
Docker available to the cron user. New commits are normally detected on the
next minute tick; build and health-check time is additional. Preview branches,
remote CI deployment, and automatic rollback are outside this implementation.

Regression tests: `python3 -m unittest discover -s tests -p 'test_deploy.py' -v`.

## Backend connection and branches

Production testbed and API: **https://troute.mangagaki.net**. Only `main` is
deployed here, on the Mac mini. The existing Cloudflare Tunnel routes this
hostname to the existing jsb1 edge Caddy. Caddy serves the testbed from its
loopback host port 18081, proxies `/api/*` to the API on port 18080 after
stripping `/api`, and preserves the direct `/health` and `/optimize` API paths.
The containers still listen on port 8080. TLS and credentials stay in the
existing host infrastructure; no new tunnel or certificate is required.

```sh
curl --fail --show-error https://troute.mangagaki.net/health
```

Expected: HTTP 200 and `{"status":"ok"}`. After the HTTP integration is merged
to main, the same host also serves `POST /optimize`. `/` serves the testbed,
whose same-origin requests use `/api/health` and, when configured,
`/api/optimize`. The testbed also remains directly available on this Mac at
`http://127.0.0.1:18081`.

The production checkout is `$HOME/services/troute`. Its ignored `.env` fixes
`COMPOSE_PROJECT_NAME=troute`, `TROUTE_PORT=8080`, `TROUTE_HOST_PORT=18080`, and
`TROUTE_TESTBED_PORT=18081`. Run watcher status and Compose commands there.
The one-minute cron watcher rebuilds both services when `origin/main` changes.
Build time is additional to polling time.

### Existing edge setup and diagnostics

`deploy/troute.caddy` is the non-secret site configuration used by this Mac's
existing jsb1 edge. It relies on that edge's `jsb_tls` snippet and
`host.docker.internal` mapping. It is not a standalone Caddy configuration.
At initial setup it is copied as `_troute-main.caddy` into the edge's existing
mounted routes directory, then the full Caddy configuration is validated and
gracefully reloaded. That local route is independent of jsb1 branch routes.
Cloudflare DNS has an explicit proxied CNAME for `troute.mangagaki.net` to the
existing **jsb1** tunnel; the zone's wildcard points to another tunnel and is
left unchanged. The jsb1 tunnel's existing wildcard ingress already reaches
Caddy, so its configuration does not need changing or restarting.

Normal troute deployment changes containers only. It does not rewrite edge
configuration, DNS, certificates, or tunnels. If the host port changes, update
the installed Caddy route and validate/reload the existing edge once. Never
point this route at a development/preview checkout.

Check the layers separately (the local commands below use this Mac's ports):

```sh
# Application: required by deploy.sh, independently of Cloudflare.
curl --fail --show-error http://127.0.0.1:18080/health
# Existing Caddy listener with the production hostname.
# -k is only for this local origin-certificate diagnostic, never public HTTPS.
curl --fail --show-error --insecure \
  --resolve troute.mangagaki.net:4443:127.0.0.1 \
  https://troute.mangagaki.net:4443/health
# Public TLS, DNS, and tunnel: a separate external verification.
curl --fail --show-error https://troute.mangagaki.net/health
docker compose logs --tail 50 troute
docker logs --tail 50 jsb1-edge-edge-1
docker logs --tail 50 jsb1-edge-tunnel-1
```

Local failure indicates an application/Docker problem. Local success with
proxy failure indicates the edge route/upstream. Local and proxy success with
public failure indicates DNS/tunnel/external connectivity. External availability
does not control local deployment success; a Cloudflare outage must not trigger
repeated application rebuilds. Both local API and testbed checks remain required.

Browser and curl HTTPS checks pass. The existing Cloudflare security policy
returns HTTP 403 / error 1010 for Python's default `Python-urllib` User-Agent.
Cloudflare documents this as a [browser-signature block](https://developers.cloudflare.com/support/troubleshooting/http-status-codes/cloudflare-1xxx-errors/error-1010/).
Check the actual backend HTTP client when integrating; local server-to-server
access avoids this edge policy. No zone-wide security or WAF rules were changed
as part of domain setup.

For a Trasolve backend outside the Mac's Docker network:

```dotenv
TROUTE_URL=https://troute.mangagaki.net
```

This is a server-to-server setting; Trasolve browsers continue to call their
backend. No API CORS changes or testbed public access are needed.

The main branch is **`main`**. Production uses only the troute instance built
from `main`. The Trasolve backend selects its instance through `TROUTE_URL`,
not through a branch field in an API request.

When the backend joins the same Docker network as troute:

```dotenv
TROUTE_URL=http://troute:8080
```

The default Compose network is isolated from other Compose projects. To use
the service hostname from Trasolve, connect its backend to the same network.
A backend running directly on this host can instead use
`TROUTE_URL=http://127.0.0.1:18080`.
`TROUTE_URL` belongs to the backend configuration, not the troute server.

For future previews, build each branch from a separate checkout and run it as
a separate Compose project/container with a distinct host port if published.
There is no fixed `container_name`, so Compose project names can separate the
instances. If instances share a backend network, give them unique network
aliases such as `troute-main` and `troute-w1-jjs`, and configure each backend's
`TROUTE_URL` accordingly. Never switch branches inside a running process.
Preview deployment automation is outside the current scope.

## Secrets

`.env` and `.env.*` are excluded from Git and the Docker build context;
`.env.example` contains only safe defaults. Do not commit keys or tokens.
No Google Maps key is needed for the deterministic development provider. Add
`GOOGLE_MAPS_API_KEY` through environment configuration only when an actual
provider integration requires it.

## Project Status

Early development. The v0 types, component boundaries, health and optimize HTTP
endpoints, and container environment are in place. The optimize endpoint uses
temporary deterministic provider/solver implementations; production routing
data and optimization algorithms are pending.
