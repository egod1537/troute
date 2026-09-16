#!/usr/bin/env bash
set -euo pipefail

export PATH="$PATH:$HOME/.docker/bin:$HOME/.orbstack/bin:/opt/homebrew/bin:/usr/local/bin"
export GIT_TERMINAL_PROMPT=0

log() { printf '[%s] %s\n' "$(date '+%Y-%m-%d %H:%M:%S%z')" "$*"; }
fail() { log "$*" >&2; exit 1; }
value() { if [[ -f "$state_dir/$1" ]]; then cat "$state_dir/$1"; fi; }
record() {
  printf '%s\n' "$2" > "$state_dir/$1.tmp.$$"
  mv -f -- "$state_dir/$1.tmp.$$" "$state_dir/$1"
}
remote_main() {
  local result sha ref
  result=$(git ls-remote --exit-code --heads origin refs/heads/main) || return 1
  IFS=$'\t' read -r sha ref <<< "$result"
  [[ "$sha" =~ ^[0-9a-f]{40}$ && "$ref" == refs/heads/main ]] || return 1
  printf '%s\n' "$sha"
}

run_once() {
  [[ "$(git branch --show-current)" == main ]] || fail 'Run deployments only from a dedicated main checkout.'
  # A separate scan lock protects state and prevents overlapping cron polls;
  # deploy.sh retains its lock to exclude direct/manual deployments as well.
  if ! mkdir "$state_dir/watcher.lock" 2>/dev/null; then
    log 'Watcher already running (or stale lock needs inspection); skipping'
    return 0
  fi
  trap 'rm -f -- "$state_dir/watcher.lock/pid"; rmdir -- "$state_dir/watcher.lock"' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  trap 'exit 129' HUP
  printf '%s\n' "$$" > "$state_dir/watcher.lock/pid"

  local sha current attempted retry_at now result
  if ! sha=$(remote_main); then
    log 'Could not read origin/main; no deployment attempted'
    return 1
  fi
  if [[ -d "$(git rev-parse --git-common-dir)/troute-deploy.lock" ]]; then
    log 'Deployment lock is held; skipping'
    return 0
  fi
  current=$(value deployed-commit)
  if [[ "$current" == "$sha" && "$(value last-status)" == succeeded ]]; then
    return 0
  fi
  attempted=$(value last-attempted-commit)
  retry_at=$(value next-retry-at)
  now=$(date +%s)
  if [[ "$attempted" == "$sha" && "$retry_at" =~ ^[0-9]+$ ]] && (( now < retry_at )); then
    log "Waiting to retry main at $sha"
    return 0
  fi

  log "Detected origin/main at $sha; starting deployment"
  record last-attempted-commit "$sha"
  record next-retry-at "$((now + retry_seconds))"
  record last-status deploying
  # This is a separate Bash process: failures inside deploy.sh retain errexit.
  result=0
  # State files remain private, but deployment checkout/build artifacts should
  # not inherit the watcher's restrictive umask (public files must be readable).
  (
    umask 022
    /bin/bash "$repo_root/scripts/deploy.sh" "$repo_root" "$sha"
  ) || result=$?
  if (( result == 0 )); then
    record deployed-commit "$sha"
    record next-retry-at 0
    record last-status succeeded
    log "Deployment succeeded: main at $sha"
  elif (( result == 75 )); then
    record next-retry-at 0
    record last-status superseded
    log 'Remote main advanced; checking again next minute'
  else
    record next-retry-at "$(($(date +%s) + retry_seconds))"
    record last-status failed
    log "Deployment failed: main at $sha (exit $result; retry in ${retry_seconds}s)"
    return "$result"
  fi
}

# POSIX shell quoting for cron's /bin/sh, including spaces and apostrophes.
quote() {
  local remaining=$1
  printf "'"
  while [[ "$remaining" == *"'"* ]]; do
    printf '%s%s' "${remaining%%\'*}" "'\\''"
    remaining=${remaining#*\'}
  done
  printf "%s'" "$remaining"
}

read_crontab() {
  local error_file
  error_file=$(mktemp "${TMPDIR:-/tmp}/troute-crontab-error.XXXXXX")
  if ! cron_contents=$(LC_ALL=C crontab -l 2> "$error_file"); then
    if ! grep -Fq 'no crontab for' "$error_file"; then
      cat "$error_file" >&2
      rm -f -- "$error_file"
      fail 'Cannot read crontab; leaving it unchanged'
    fi
    cron_contents=''
  fi
  rm -f -- "$error_file"
}

edit_crontab() {
  local operation=$1 next_crontab cron_line
  read_crontab
  next_crontab=$(mktemp "${TMPDIR:-/tmp}/troute-crontab.XXXXXX")
  trap 'rm -f -- "$next_crontab"' EXIT
  printf '%s\n' "$cron_contents" | awk '!/# troute-auto-deploy[[:space:]]*$/' > "$next_crontab"
  if [[ "$operation" == install ]]; then
    [[ "$(git branch --show-current)" == main ]] || fail 'Install only from a dedicated main checkout.'
    if ! git diff --quiet || ! git diff --cached --quiet; then
      fail 'Commit or discard tracked edits before installing in the deployment checkout.'
    fi
    git ls-files --error-unmatch auto-deploy.sh scripts/deploy.sh > /dev/null \
      || fail 'Publish the watcher on main before installing.'
    [[ "$(git rev-parse HEAD)" == "$(remote_main)" ]] \
      || fail 'Update the dedicated checkout to the published origin/main before installing.'
    [[ "$repo_root" != *$'\n'* && "$repo_root" != *$'\r'* ]] || fail 'Newlines in the checkout path are not supported.'
    cron_line="* * * * * TROUTE_AUTO_DEPLOY_RETRY_SEC=$retry_seconds /bin/bash $(quote "$repo_root/auto-deploy.sh") --once >> $(quote "$log_file") 2>&1 # troute-auto-deploy"
    # Cron interprets percent signs before invoking the shell, even in quotes.
    cron_line=${cron_line//%/\\%}
    printf '%s\n' "$cron_line" >> "$next_crontab"
  fi
  crontab "$next_crontab"
  rm -f -- "$next_crontab"
  trap - EXIT
  printf 'Watcher %s complete. Branch: main. Log: %s\n' "$operation" "$log_file"
}

show_status() {
  local remote cron_script
  read_crontab
  cron_script=$(quote "$repo_root/auto-deploy.sh")
  cron_script=${cron_script//%/\\%}
  if [[ "$cron_contents" == *'# troute-auto-deploy'* ]]; then
    if [[ "$cron_contents" == *"$cron_script"* ]]; then
      printf 'Watcher: installed (every minute; one troute entry per user)\n'
    else
      printf 'Watcher: installed for another checkout; showing local checkout state below\n'
    fi
  else
    printf 'Watcher: not installed\n'
  fi
  if ! remote=$(remote_main); then remote=unavailable; fi
  printf 'Tracked branch: main\nCheckout: %s\n' "$repo_root"
  printf 'Deployed commit: %s\nOrigin/main commit: %s\n' "$(value deployed-commit)" "$remote"
  printf 'Last deployment: %s\nLast attempted commit: %s\n' "$(value last-status)" "$(value last-attempted-commit)"
  printf 'Next retry epoch: %s\nLog: %s\n' "$(value next-retry-at)" "$log_file"
  if [[ -d "$state_dir/watcher.lock" ]]; then printf 'Watcher lock: present\n'; fi
  if [[ -d "$(git rev-parse --git-common-dir)/troute-deploy.lock" ]]; then
    printf 'Deployment lock: present\n'
  fi
}

main() {
  [[ $# -eq 1 ]] || fail 'Usage: auto-deploy.sh --once | --install | --status | --uninstall'
  repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
  cd -- "$repo_root"
  [[ "$(git rev-parse --show-toplevel)" == "$repo_root" ]] || fail 'Watcher must be at the checkout root.'
  state_dir="$(git rev-parse --absolute-git-dir)/troute-deploy"
  log_file="$state_dir/logs/auto-deploy.log"
  retry_seconds=${TROUTE_AUTO_DEPLOY_RETRY_SEC:-300}
  [[ "$retry_seconds" =~ ^[0-9]{1,9}$ ]] || fail 'TROUTE_AUTO_DEPLOY_RETRY_SEC must be a non-negative integer (at most 9 digits).'
  retry_seconds=$((10#$retry_seconds))
  umask 077
  mkdir -p "$state_dir/logs"
  case "$1" in
    --once) run_once >> "$log_file" 2>&1 ;;
    --install) edit_crontab install ;;
    --uninstall) edit_crontab uninstall ;;
    --status) show_status ;;
    *) fail 'Usage: auto-deploy.sh --once | --install | --status | --uninstall' ;;
  esac
}

# All functions are parsed before a deployment can update this file on disk.
main "$@"
