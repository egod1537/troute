#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repository_root"

cargo test --test fixture_loader

if [[ "${RUN_REAL_ROUTE_TESTS:-0}" == "1" ]]; then
  cargo test --test tcache_integration -- --ignored --nocapture
fi
