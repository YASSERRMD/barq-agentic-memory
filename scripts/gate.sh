#!/usr/bin/env bash
# Phase gate for barq-agentic-memory.
# Output is recorded in docs/phase-log.md; it is evidence, not a merge
# blocker (per temp/git_instruction.md).
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== cargo fmt --check =="
cargo fmt --all -- --check
echo "fmt: OK"

echo "== cargo clippy -D warnings =="
cargo clippy --workspace --all-targets --quiet -- -D warnings
echo "clippy: OK"

echo "== cargo test =="
cargo test --workspace --quiet
echo "test: OK"

echo "GATE PASSED"
