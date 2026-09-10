#!/usr/bin/env bash
set -euo pipefail

# Non-interactive macOS SSH sessions may not load Homebrew/OrbStack paths.
export PATH="$PATH:$HOME/.docker/bin:$HOME/.orbstack/bin:/opt/homebrew/bin:/usr/local/bin"
export GIT_TERMINAL_PROMPT=0

fail() {
  printf 'Deployment failed: %s\n' "$1" >&2
  exit 1
}

[[ $# -eq 1 && "$1" == /* ]] || fail 'Provide an absolute deployment checkout path.'
echo 'SSH connected; checking deployment prerequisites'
cd -- "$1"
[[ "$(git rev-parse --show-toplevel)" == "$(pwd -P)" ]] || fail 'Path must be the repository root.'
command -v curl > /dev/null
docker compose version > /dev/null
docker info > /dev/null

# Hold a host-side lock too: a timed-out/disconnected runner can leave its SSH
# command running. Never overlap it with a second deployment. A leftover lock
# requires an operator to confirm no deployment is running before removing it.
lock_dir="$(git rev-parse --git-common-dir)/troute-deploy.lock"
mkdir "$lock_dir" 2>/dev/null || fail 'Another deployment is running, or its deployment lock needs inspection.'
response_file=''
# Invoked indirectly by the EXIT trap.
# shellcheck disable=SC2329
cleanup() {
  if [[ -n "$response_file" ]]; then rm -f -- "$response_file"; fi
  rmdir -- "$lock_dir"
}
trap 'cleanup' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
trap 'exit 129' HUP

echo 'Fetching origin/main'
git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main

# reset --hard can overwrite an untracked file if the fetched commit tracks
# the same path. Refuse runtime env files in either index before resetting.
protected_files=$( { git ls-files -- '.env' '.env.*'; git ls-tree -r --name-only origin/main -- '.env' '.env.*'; } )
while IFS= read -r file; do
  case "$file" in
    ''|.env.example) ;;
    *) fail 'A runtime .env file is tracked; refusing to overwrite production configuration.' ;;
  esac
done <<< "$protected_files"

git checkout --force main
git reset --hard origin/main
[[ "$(git branch --show-current)" == main ]] || fail 'Checkout is not on main.'
echo "Repository updated to main at $(git rev-parse --short HEAD)"

# Build must succeed before Compose replaces the existing container.
echo 'Building troute image'
docker compose build troute
echo 'Starting troute container'
docker compose up -d --no-build troute

container_id=$(docker compose ps --all --quiet troute)
[[ -n "$container_id" && "$container_id" != *$'\n'* ]] || fail 'Expected exactly one troute container.'
# Discover the real published port, including TROUTE_HOST_PORT from .env,
# without sourcing .env or printing resolved configuration/secrets.
host_port=$(docker inspect --format '{{range .NetworkSettings.Ports}}{{range .}}{{if eq .HostIp "127.0.0.1"}}{{println .HostPort}}{{end}}{{end}}{{end}}' "$container_id")
[[ "$host_port" =~ ^[0-9]{1,5}$ ]] || fail 'Expected one port published on 127.0.0.1.'
response_file=$(mktemp "${TMPDIR:-/tmp}/troute-health.XXXXXX")
health_pattern='^[[:space:]]*\{[[:space:]]*"status"[[:space:]]*:[[:space:]]*"ok"[[:space:]]*\}[[:space:]]*$'

echo 'Checking /health on the deployment host (up to 20 attempts)'
for ((attempt = 1; attempt <= 20; attempt++)); do
  status=''
  if status=$(curl --silent --show-error --noproxy '*' \
      --connect-timeout 2 --max-time 5 \
      --output "$response_file" --write-out '%{http_code}' \
      "http://127.0.0.1:$host_port/health"); then
    if [[ "$status" == 200 && "$(< "$response_file")" =~ $health_pattern ]]; then
      echo 'Deployment succeeded: /health returned HTTP 200 and status ok'
      exit 0
    fi
  fi
  echo "Health check attempt $attempt/20 failed"
  if (( attempt < 20 )); then sleep 3; fi
done
fail '/health did not return HTTP 200 with status ok within the retry limit.'
