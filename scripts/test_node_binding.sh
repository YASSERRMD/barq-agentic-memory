#!/usr/bin/env bash
# Opt-in Node.js binding verification: builds the native addon via
# napi-rs CLI and runs an end-to-end smoke test.
# Requires: node >= 20, npm.
set -euo pipefail
cd "$(dirname "$0")/../crates/bindings/memory-bindings-node"
[ -d node_modules ] || npm install --no-fund --no-audit
npx napi build --release --platform
node smoke.mjs
