#!/usr/bin/env bash
# One command that runs formatting, linting, unit tests, and integration
# tests, exactly as CI and the evaluator do.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --all-targets --locked -- -D warnings

echo "==> cargo test"
cargo test --locked

echo "OK"
