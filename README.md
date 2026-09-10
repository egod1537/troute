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
   +-- API types
   +-- Routing Provider
   +-- Travel Time Matrix
   +-- Route Solver
   +-- Schedule Calculation
```

The component interfaces and v0 data flow are defined. The HTTP server currently
exposes only `GET /health`; route optimization, Google Maps integration, and
caching are not implemented. Trasolve user browsers call the Trasolve backend, which calls
troute over HTTP. The separate developer testbed uses its own same-origin API proxy.

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

`testbed/` is a one-page React + TypeScript + Vite tool for developers, separate
from the Rust API and the Trasolve user interface. It has a JSON input editor,
validation, API health status, route-result/table components, formatted raw
responses, HTTP timing, and an in-memory graph of the last 40 requests.

**The Rust API currently implements only `GET /health`.** The primary action
therefore runs a health check without submitting the editor payload. No solver
runs and no route or solver timing is fabricated. Sample Place IDs illustrate
the v0 input shape only. Once a real route endpoint exists, configure
`VITE_TROUTE_ROUTE_PATH` with that endpoint's relative path; the client posts the
v0 input and renders `route` / `total_travel_minutes` from the existing DTOs.
The route path is deliberately unset by default. Error status and response
bodies remain visible, including malformed JSON and infeasible-route errors.

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

`request_latency_ms` measures the browser round trip, including the proxy and
response transfer. The current API provides no `solver_latency_ms`, so it is
shown as unavailable. Measurements exist only in browser memory and disappear
on refresh; there is no benchmark DB or frontend solver implementation.

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

Production API: **https://troute.mangagaki.net**. Only `main` is deployed here,
on the Mac mini. The existing Cloudflare Tunnel routes this hostname to the
existing jsb1 edge Caddy, which forwards to the API's loopback host port 18080.
The container still listens on port 8080. TLS and credentials stay in the
existing host infrastructure; no new tunnel or certificate is required.

```sh
curl --fail --show-error https://troute.mangagaki.net/health
```

Expected: HTTP 200 and `{"status":"ok"}`. `/` is currently HTTP 404: this host
serves the API, not the testbed. The developer testbed stays private on
`http://127.0.0.1:18081` on this Mac; no testbed DNS record is configured.

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
No Google Maps key is needed now. Add `GOOGLE_MAPS_API_KEY` through environment
configuration only when an actual provider integration requires it.

## Project Status

Early development. The v0 types, component boundaries, HTTP health endpoint,
and container environment are in place. Routing provider and optimization API
implementations are pending.
