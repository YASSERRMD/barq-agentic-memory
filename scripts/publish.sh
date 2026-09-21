#!/usr/bin/env bash
# Ordered crates.io publishing. Dependency order matters: every crate
# consumes the ones above it, which must already exist on the registry.
# Run `cargo login` before the real thing.
#
# `--dry-run` fully verifies only the first (leaf) crate — later crates
# resolve their internal deps from crates.io, which are absent until
# the earlier publishes land. To validate every manifest up front:
#   for c in <crates>; do cargo package --list -p $c >/dev/null; done
#
# The two binding crates are NOT publishable from this workspace (they
# are excluded and ship as wheels/addons via maturin / napi instead).
set -euo pipefail
cd "$(dirname "$0")/.."

DRY="${1:---dry-run}"

ORDER=(
  memory-domain
  memory-provider-api
  provider-local
  provider-postgres
  provider-redis
  provider-pgvector
  memory-retrieval
  memory-classifier
  memory-dedup
  memory-conflict
  memory-episodic
  memory-graph
  memory-procedural
  memory-prospective
  memory-lifecycle
  memory-policy
  memory-reliability
  memory-workers
  memory-core
  memory-server
  memory-client
)

for crate in "${ORDER[@]}"; do
  echo "==> cargo publish -p $crate $DRY"
  # shellcheck disable=SC2086
  cargo publish -p "$crate" $DRY
done

echo "Publish order complete ($DRY)."
