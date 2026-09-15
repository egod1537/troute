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
   +-- Routing Provider
   +-- Travel Time Matrix
   +-- Route Solver
   +-- Schedule Calculation
```

The component interfaces and v0 data flow are defined. The HTTP server exposes
`GET /health`, `POST /optimize`, and the reverse-integration verification endpoint
`GET /integration/trasolve/health`. The optimize endpoint executes the existing
provider -> solver -> schedule service pipeline. Its currently wired provider
and solver are deterministic development placeholders; real travel-time lookup,
actual optimization, Google Maps integration, and caching are not implemented.
Trasolve user browsers call the Trasolve backend, which calls troute over HTTP.
The separate developer testbed uses its own same-origin API proxy.

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
- [x] Developer testbed with health checks, input editor, and request timing
- [ ] Distance matrix generation
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
set variables in the shell for `cargo run`.

`TRASOLVE_BASE_URL` optionally configures reverse server-to-server calls to the
Trasolve backend. It has no implicit default. A missing value leaves troute and
its existing endpoints available; an invalid configured URL fails startup with
a clear configuration error. Trailing slashes are normalized. The outbound
client uses a five-second timeout and is reused across requests.

## HTTP API

`POST /optimize` accepts `application/json`. Time values are strict 24-hour
`HH:MM` strings on both request and response; numeric minute values and forms
such as `9:00`, `24:00`, or `09:60` are rejected. Requests may contain 1 to 500
locations. The required `job_id` is an opaque correlation value supplied by
Trasolve; it must be non-blank and at most 128 characters. Location IDs and
Place IDs must be non-blank strings of at most 512 characters, location IDs
must be unique, and `start_location_id` must match a location. Overnight
windows are not supported, so `open_time` must not be later than `close_time`.
The request body limit is 1 MiB.

```sh
curl -i -X POST http://127.0.0.1:8080/optimize \
  -H 'Content-Type: application/json' \
  -d '{
    "job_id": "route-local-example",
    "locations": [
      {
        "id": "place-1",
        "place_id": "GOOGLE_PLACE_ID_1",
        "open_time": "09:00",
        "close_time": "18:00",
        "stay_minutes": 60
      },
      {
        "id": "place-2",
        "place_id": "GOOGLE_PLACE_ID_2",
        "open_time": "10:00",
        "close_time": "18:00",
        "stay_minutes": 45
      }
    ],
    "start_location_id": "place-1",
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
      "location_id": "place-1",
      "order": 2,
      "arrival_time": "11:00"
    }
  ],
  "total_travel_minutes": 30
}
```

The start location is the v0 depot: its stay duration and opening window are
ignored for the initial departure and final return. A missing
`departure_time` is omitted rather than serialized as `null`.

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
`DevelopmentRouteSolver` starts at the selected location, visits all other
locations in request order, and returns to the start. This behavior is
deterministic and exercises the real `RouteOptimizationService` and schedule,
but it is **not route optimization and does not use Google travel data**. The
implementations are isolated in `src/development.rs` so they can be replaced
without changing the HTTP handler or wire contract. Schedule infeasibility is
returned as an error; the placeholders do not alter request semantics or invent
a successful route.

### Trasolve integration

The supported call path is:

```text
Trasolve browser/client
  -> POST /api/troute/optimize on the Trasolve backend
  -> POST /optimize on troute
  -> RouteOptimizationService
  -> provider -> solver -> schedule
```

Run troute on port 8080, then start the Trasolve backend with:

```sh
TROUTE_BASE_URL=http://127.0.0.1:8080 npm run dev -w @trasolve/backend
```

`POST http://127.0.0.1:43127/api/troute/optimize` accepts the same request body.
The Trasolve browser never connects directly to troute. The public
`POST https://troute.mangagaki.net/optimize` endpoint will become available
after this `impl` change is reviewed, merged to `main`, and deployed by the
existing main watcher.

### Reverse Trasolve connectivity

troute can independently verify the reverse communication path:

```text
external caller
  -> GET /integration/trasolve/health on troute
  -> TrasolveClient
  -> GET /api/internal/troute/health on Trasolve
  -> troute response
```

This is connectivity infrastructure only. It does not fetch Trip data, send
optimization results, or make `RouteOptimizationService`, the routing provider,
the solver, schedule calculation, or `/optimize` depend on Trasolve.

Start the Trasolve backend from its repository, verify it directly, and then
start troute with its base URL:

```sh
# In the Trasolve repository (impl branch):
npm run dev -w @trasolve/backend

# In separate terminals:
curl -i http://127.0.0.1:43127/api/internal/troute/health

TRASOLVE_BASE_URL=http://127.0.0.1:43127 cargo run
curl -i http://127.0.0.1:8080/integration/trasolve/health
```

The integration endpoint returns HTTP 200 when Trasolve returns its expected
typed contract:

```json
{
  "status": "ok",
  "trasolve": {
    "status": "ok",
    "service": "trasolve"
  }
}
```

When `TRASOLVE_BASE_URL` is absent, the endpoint returns HTTP 503 with
`TRASOLVE_NOT_CONFIGURED`; troute still starts and `/health` and `/optimize`
continue to work. Timeouts return HTTP 504 with `TRASOLVE_TIMEOUT`. Connection
failures and upstream 5xx responses return HTTP 503. Unexpected response bodies
and upstream contract mismatches return HTTP 502. All failures use the existing
JSON error envelope. No browser CORS or service authentication is added.

### Job event contract foundation

Trasolve supplies `job_id` in each optimize request. troute treats it as an
opaque identifier and must use the same value to correlate future callbacks:

```text
POST {TRASOLVE_BASE_URL}/api/internal/troute/jobs/{job_id}/events
```

The reusable callback sender posts this common JSON envelope:

```json
{
  "sequence": 1,
  "type": "progress",
  "data": {}
}
```

Event types are exactly `progress`, `error`, and `result`. Sequence numbers are
per job, begin at 1, and increment for each event; there is no process-wide
sequence.

### Progress and error callbacks

When `TRASOLVE_BASE_URL` is configured, `/optimize` reports these coarse,
real pipeline boundaries in order:

- `accepted` at 0: the request entered the optimization pipeline;
- `building_matrix` at 20: travel-time matrix construction is starting;
- `solving` at 60: visit-order optimization is starting;
- `scheduling` at 85: itinerary schedule construction is starting.

These values identify stages, not solver iterations. Progress never reports
100; a terminal `result` event represents successful completion.

Failures produce an `error` event whose data contains `code`, `message`, and
`detail`. Stable codes are `INVALID_REQUEST`, `ROUTING_UNAVAILABLE`,
`NO_FEASIBLE_ROUTE`, `SOLVER_ERROR`, and `SCHEDULE_ERROR`. Solver and schedule
feasibility failures both use `NO_FEASIBLE_ROUTE`; unexpected failures retain
their component-specific code. Details contain concise error text and never a
Rust backtrace.

Callback delivery is best-effort relative to the optimize HTTP response.
Events are queued synchronously, delivered sequentially by an asynchronous
sender, and drained for a bounded period before the response returns. A
callback timeout or rejection is logged but never replaces the optimization
result or its original error. Without `TRASOLVE_BASE_URL`, reporting is a no-op
and optimization remains fully available. Solver behavior is unchanged.

### Successful result lifecycle

After routing, solving, and scheduling succeed, troute constructs the same
`OptimizeRouteResponse` returned by `/optimize` and queues it as the final event:

```json
{
  "sequence": 5,
  "type": "result",
  "data": {
    "route": [],
    "total_travel_minutes": 0
  }
}
```

The `data` value uses the canonical optimize response type rather than a
separate callback DTO. A successful request therefore follows this lifecycle:

```text
Trasolve creates job_id
  -> troute receives POST /optimize
  -> accepted -> building_matrix -> solving -> scheduling
  -> result callback
  -> synchronous OptimizeRouteResponse
```

`result` is emitted exactly once and last. Error paths emit `error` instead and
never emit `result`. As with progress and error delivery, a failed or timed-out
result callback is logged but does not change a successful HTTP response.

For local end-to-end verification, run Trasolve and troute with reciprocal base
URLs, submit an optimization through Trasolve, then inspect the returned job:

```sh
curl -i -X POST http://127.0.0.1:43127/api/troute/optimize \
  -H 'Content-Type: application/json' \
  --data @optimize-request.json

curl -i http://127.0.0.1:43127/api/internal/troute/jobs/JOB_ID
```

After successful delivery, the job is expected to be `completed` with progress
100, a populated result, and an event history containing the coarse progress
events followed by the result event. The Trasolve job endpoint owns that stored
state; troute remains synchronous and does not write Trip storage directly.

## Docker

The multi-stage Dockerfile builds the binary and copies it into a Debian slim
runtime image. It runs as a non-root user without the Rust toolchain.

```sh
docker compose build
docker compose up -d
curl -i http://localhost:8080/health
docker compose logs -f troute
docker compose down
```

To build and run in the foreground, use `docker compose up --build`. To rebuild
and run in the background, use `docker compose up -d --build`.

Compose publishes the host port on `127.0.0.1` only. The application still binds
to `0.0.0.0` inside the container, so containers on its Docker network can reach
it. Both application and host ports default to `8080`.

Compose passes through `TRASOLVE_BASE_URL` when it is set. Inside the troute
container, `127.0.0.1` refers to that container, not the Docker host or a
Trasolve container. Set the variable to an address actually reachable from the
troute container, such as the Trasolve Compose service name when both services
share a network. The source code makes no host-specific networking assumption.

If the host port is already occupied, create a local configuration:

```sh
cp .env.example .env
```

Set `TROUTE_HOST_PORT=18080` in `.env`, then run `docker compose up -d --build`
and check `http://localhost:18080/health`. `TROUTE_HOST_PORT` changes only the
published host port; `TROUTE_PORT` sets the application/container port. Compose
automatically reads `.env` and keeps the port mapping consistent.

No API container healthcheck is configured: the slim runtime has no HTTP client,
and adding one solely for this check is unnecessary at this stage. Verify
`/health` from the host or the Trasolve backend. The restart policy restarts an
exited container; it does not detect an unresponsive process.

## Developer testbed

`testbed/` is a compact Blueprint + React + TypeScript + Vite API playground for
developers, separate from the Rust API and the Trasolve user interface. It
provides API health, an editable optimize request, request validation, route
execution, a structured route result, the raw response, HTTP status, and the
latest round-trip latency.

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
the proxy and response transfer. It is not solver execution time and is not
stored as history or persisted.

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
and the testbed `/` to return HTTP 200. Both published host ports are discovered
from their containers, including `.env` overrides. A testbed failure marks the
deployment failed without stopping the API.
Health checks retry up to 20 times at 3-second intervals with a 5-second request
timeout. Only success updates `deployed-commit`. The same failed commit waits
300 seconds after failure before retrying; a newer commit bypasses that delay.
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
`TROUTE_URL=http://localhost:8080` (or the configured host port).
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
