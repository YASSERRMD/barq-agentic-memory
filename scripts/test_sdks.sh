#!/usr/bin/env bash
# Opt-in SDK verification: boots the REST server, then exercises the
# Rust, Python, TypeScript, and .NET clients against it.
set -euo pipefail
cd "$(dirname "$0")/.."

export BARQ_ADDR=127.0.0.1:18099
export BARQ_BASE=http://127.0.0.1:18099

cargo build -p memory-server --quiet
./target/debug/memory-server > /tmp/barq-sdk-server.log 2>&1 &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true' EXIT
sleep 2

echo "== rust sdk =="
cargo run -q -p memory-client --example live_check

echo "== python sdk =="
python3 sdks/python/smoke.py

echo "== typescript sdk =="
node sdks/typescript/smoke.mjs

echo "== dotnet sdk =="
dotnet run --project sdks/dotnet/smoke -v q

echo "ALL SDK SMOKE TESTS OK"
