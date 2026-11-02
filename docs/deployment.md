# Deployment

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

## Mac mini operation

Run `docker compose up -d --build` from the `main` checkout. Both services use
`restart: always`, so Docker starts them again after its daemon restarts, including
when they had been stopped before a Mac reboot. Configure the installed Docker
engine to start automatically after reboot/login, and keep the Mac awake for
continuous service. Compose does not start the Docker engine or wake the Mac.
An explicitly stopped container stays stopped until it is started manually or the
Docker daemon restarts; use `docker compose up -d` to start it immediately.
`docker compose down` removes it.

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
