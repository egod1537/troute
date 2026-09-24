#!/usr/bin/env bash
set -euo pipefail

cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --test algorithm_validation
cargo test --test refactor_regression
cargo test --bench clustered_solver
