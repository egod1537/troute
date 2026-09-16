#!/usr/bin/env bash
set -euo pipefail

# Cron on macOS may not load Homebrew/OrbStack paths.
export PATH="$PATH:$HOME/.docker/bin:$HOME/.orbstack/bin:/opt/homebrew/bin:/usr/local/bin"
export GIT_TERMINAL_PROMPT=0

github_status_disabled_logged=false

set_github_status() {
  local sha=$1 state=$2 description=$3 payload
  if [[ -z "${GITHUB_TOKEN:-}" ]]; then
    if [[ "$github_status_disabled_logged" != true ]]; then
      echo 'GitHub deployment status reporting disabled'
      github_status_disabled_logged=true
    fi
    return 0
  fi

  printf -v payload \
    '{"state":"%s","context":"deploy/troute","description":"%s","target_url":"https://troute.mangagaki.net"}' \
    "$state" "$description"
  # Read the authorization header through curl's stdin config so the token is
  # neither exposed in argv nor written to deployment logs.
  if ! printf 'header = "Authorization: Bearer %s"\n' "$GITHUB_TOKEN" | \
      curl --silent --show-error --fail \
        --config - \
        --connect-timeout 5 \
        --max-time 10 \
        --request POST \
        --header 'Accept: application/vnd.github+json' \
        --header 'X-GitHub-Api-Version: 2022-11-28' \
        --header 'Content-Type: application/json' \
        --output /dev/null \
        --data "$payload" \
        "https://api.github.com/repos/egod1537/troute/statuses/$sha"; then
    printf 'Warning: GitHub deployment status update failed (%s).\n' "$state" >&2
  fi
  return 0
}

fail() {
  printf 'Deployment failed: %s\n' "$1" >&2
  exit 1
}

check_service() {
  local service=$1 endpoint=$2 kind=$3 container_id host_port status attempt
  local health_pattern='^[[:space:]]*\{[[:space:]]*"status"[[:space:]]*:[[:space:]]*"ok"[[:space:]]*\}[[:space:]]*$'
  container_id=$(docker compose ps --all --quiet "$service")
  [[ -n "$container_id" && "$container_id" != *$'\n'* ]] || fail "Expected exactly one $service container."
  # Discover the actual host port without sourcing or printing runtime .env.
  host_port=$(docker inspect --format '{{range .NetworkSettings.Ports}}{{range .}}{{if eq .HostIp "127.0.0.1"}}{{println .HostPort}}{{end}}{{end}}{{end}}' "$container_id")
  [[ "$host_port" =~ ^[0-9]{1,5}$ ]] || fail "Expected one $service port published on 127.0.0.1."
  echo "Checking $service $endpoint (up to 20 attempts)"
  for ((attempt = 1; attempt <= 20; attempt++)); do
    status=''
    if status=$(curl --silent --show-error --noproxy '*' \
        --connect-timeout 2 --max-time 5 \
        --output "$response_file" --write-out '%{http_code}' \
        "http://127.0.0.1:$host_port$endpoint"); then
      if [[ "$status" == 200 ]] && { [[ "$kind" == html ]] || [[ "$(< "$response_file")" =~ $health_pattern ]]; }; then
        echo "$service $endpoint passed: HTTP 200"
        return 0
      fi
    fi
    echo "$service health check attempt $attempt/20 failed"
    if (( attempt < 20 )); then sleep 3; fi
  done
  fail "$service $endpoint did not pass its health check within the retry limit."
}

deploy_main() {
  [[ $# -ge 1 && $# -le 2 && "$1" == /* ]] || fail 'Provide an absolute deployment checkout path and optional expected main SHA.'
  expected_sha=${2:-}
  [[ -z "$expected_sha" || "$expected_sha" =~ ^[0-9a-f]{40}$ ]] || fail 'Invalid expected main SHA.'
  echo 'Checking deployment prerequisites'
  cd -- "$1"
  [[ "$(git rev-parse --show-toplevel)" == "$(pwd -P)" ]] || fail 'Path must be the repository root.'
  command -v curl > /dev/null
  docker compose version > /dev/null
  docker info > /dev/null

  # This also excludes a manual deployment while the watcher is deploying.
  lock_dir="$(git rev-parse --git-common-dir)/troute-deploy.lock"
  mkdir "$lock_dir" 2>/dev/null || fail 'Another deployment is running, or its deployment lock needs inspection.'
  response_file=''
  deployed_sha=''
  deployment_started=false
  deployment_succeeded=false
  deployment_interrupted=false
  # Invoked indirectly by the EXIT trap.
  # shellcheck disable=SC2329
  cleanup() {
    local exit_code=$? failure_description='Production deployment failed'
    trap - EXIT INT TERM HUP
    if [[ "$deployment_interrupted" == true ]]; then
      failure_description='Production deployment interrupted'
    fi
    if [[ "$deployment_started" == true && "$deployment_succeeded" != true && $exit_code -ne 0 ]]; then
      set_github_status "$deployed_sha" failure "$failure_description"
    fi
    if [[ -n "$response_file" ]]; then rm -f -- "$response_file"; fi
    rmdir -- "$lock_dir" || true
    exit "$exit_code"
  }
  # Invoked indirectly by signal traps.
  # shellcheck disable=SC2329
  interrupt() {
    deployment_interrupted=true
    exit "$1"
  }
  trap 'cleanup' EXIT
  trap 'interrupt 130' INT
  trap 'interrupt 143' TERM
  trap 'interrupt 129' HUP

  echo 'Fetching origin/main'
  git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main
  if [[ -n "$expected_sha" && "$(git rev-parse origin/main)" != "$expected_sha" ]]; then
    echo 'origin/main changed during fetch; deferring to the next poll'
    exit 75
  fi

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
  deployed_sha=$(git rev-parse HEAD)
  [[ "$deployed_sha" =~ ^[0-9a-f]{40}$ ]] || fail 'Checked-out deployment SHA is invalid.'
  echo "Repository updated to main at ${deployed_sha:0:7}"

  deployment_started=true
  set_github_status "$deployed_sha" pending 'Deploying to production'

  # Build must succeed before Compose replaces the existing container.
  echo 'Building troute and testbed images'
  TROUTE_COMMIT_SHA="$deployed_sha" docker compose build
  echo 'Starting troute and testbed containers'
  docker compose up -d --no-build

  response_file=$(mktemp "${TMPDIR:-/tmp}/troute-health.XXXXXX")
  check_service troute /health json
  check_service testbed / html
  deployment_succeeded=true
  set_github_status "$deployed_sha" success 'Production deployment succeeded'
  echo 'Deployment succeeded: troute /health and testbed / returned HTTP 200'

}

# Parse the full function before reset can replace this script on disk.
deploy_main "$@"
