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
caching are not implemented. Browsers call the Trasolve backend, which calls
troute over HTTP.

## Tech Stack

- Rust
- axum / tokio
- Serde
- Docker / Docker Compose

## Development Roadmap

- [x] Basic Rust project setup
- [x] v0 request, response, location, and route data models
- [x] Travel time matrix model
- [x] Routing provider and solver interfaces
- [x] Schedule calculation for a supplied visit order
- [x] Runnable HTTP server and Docker Compose environment
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

No container healthcheck is configured: the slim runtime has no HTTP client,
and adding one solely for this check is unnecessary at this stage. Verify
`/health` from the host or the Trasolve backend. The restart policy restarts an
exited container; it does not detect an unresponsive process.

## Mac mini operation

Run `docker compose up -d --build` from the `main` checkout. The service uses
`restart: unless-stopped`, so it can restart when the Docker engine starts again.
Configure the installed Docker engine to start automatically after reboot/login,
and keep the Mac awake for continuous service. Compose does not start the Docker
engine or wake the Mac. Reboot behavior depends on that host configuration.
An explicitly stopped container remains stopped; use `docker compose up -d` to
start it again. `docker compose down` removes it.

## Automatic deployment

`.github/workflows/deploy-main.yml` deploys **main only** from a GitHub-hosted
runner over SSH to the Mac mini. A push to `main` triggers deployment; manual
`workflow_dispatch` runs also require the `main` ref. Pull requests and preview
branches do not deploy production. This becomes active after the workflow is
pushed and the connection settings below are configured.

Configure these repository **Actions Secrets** (never commit their values):

| Name | Purpose |
| --- | --- |
| `TROUTE_DEPLOY_HOST` | SSH hostname/IP reachable from the GitHub-hosted runner |
| `TROUTE_DEPLOY_USER` | Mac mini deployment user |
| `TROUTE_DEPLOY_SSH_KEY` | Dedicated, passphrase-free deployment private key |
| `TROUTE_DEPLOY_KNOWN_HOSTS` | Verified OpenSSH known_hosts entry for that host/port |
| `TROUTE_DEPLOY_PORT` | SSH port; optional, defaults to 22 |
| `TROUTE_DEPLOY_PATH` | Absolute path to a dedicated deployment clone, not a development checkout |

Port and path may instead be Actions Variables; Secrets take precedence.
Create a dedicated key pair: its public key goes in the deployment user's
`~/.ssh/authorized_keys`, its private key only in the GitHub Secret. Verify the
server host key through a trusted connection before registering known_hosts
(use `[hostname]:port` for a non-default port). Host key checking stays enabled.

The SSH user needs non-interactive Git fetch access, Bash, curl, and a running
Docker engine with Compose. Clone `main` at the configured deployment path and
keep runtime settings in its ignored `.env`. When moving an existing deployment,
preserve `TROUTE_HOST_PORT` and set `COMPOSE_PROJECT_NAME` to its existing project
name in `.env`; otherwise the new directory changes Compose's default identity.
Common macOS Docker executable paths are supplied by the script.

The workflow sends `scripts/deploy.sh` over SSH: fetch/reset to `origin/main`,
build, then `docker compose up -d --no-build troute`. **Tracked local edits in
the deployment clone are discarded.** There is no `git clean` or `.env` write;
tracked runtime `.env` files are rejected before reset.

Success requires localhost `/health` to return HTTP 200 and `{"status":"ok"}`.
The script discovers the actual host port (including `.env` overrides) and
retries up to 20 times at 3-second intervals, with a 5-second request timeout.
Any SSH, Git, build, startup, or health failure fails the Actions run. Images
build on the Mac; no public troute port, registry, or automatic rollback is used.

Concurrency keeps the running deployment and retains the latest pending run,
which fetches the latest main when it starts. `cancel-in-progress: false` avoids
assuming that cancelling SSH also stops the remote Docker build. A host-side
lock additionally refuses overlapping deployment processes. If a killed process
leaves `troute-deploy.lock` in Git's common directory, confirm no deployment is
still running before removing that empty lock directory and rerunning the workflow.

Check **Actions → Deploy troute main** for each deployment stage. Provider
secrets stay in the Mac's `.env`. A private Tailscale address needs separately
configured runner connectivity; this workflow does not join a VPN.

Script regression tests: `python3 -m unittest discover -s tests -p 'test_deploy.py' -v`.

## Backend connection and branches

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
