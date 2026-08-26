#!/usr/bin/env bash
# Criterion benchmarks — release builds only (blueprint rule).
# Quick mode keeps measurement short for CI-style recording; full runs
# should drop the flags.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo bench -p memory-core --bench engine -- --warm-up-time 1 --measurement-time 2 --sample-size 20
