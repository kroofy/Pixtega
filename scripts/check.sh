#!/usr/bin/env bash
# One command that runs formatting, linting, unit tests, and integration
# tests, exactly as CI and the evaluator do.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check

# -D warnings is also the dead-code gate: the `warnings` group includes the
# rustc dead_code and unused_* lints, and --all-targets extends it to tests.
echo "==> cargo clippy"
cargo clippy --all-targets --locked -- -D warnings

# Unused-dependency gate: exits non-zero if Cargo.toml lists a crate no
# target actually uses. Verified false positives (proc-macros, renamed
# crates) belong in [package.metadata.cargo-machete] ignored, not here.
echo "==> cargo machete"
if ! command -v cargo-machete >/dev/null 2>&1; then
    echo "error: cargo-machete is not installed." >&2
    echo "install it with: cargo install cargo-machete --locked" >&2
    exit 1
fi
cargo machete

echo "==> cargo test"
cargo test --locked

echo "OK"
