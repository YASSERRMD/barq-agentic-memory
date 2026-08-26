#!/usr/bin/env bash
# Opt-in Python binding verification: builds a real wheel via maturin
# and runs an end-to-end pytest against it.
# Usage: uv venv /tmp/barq-py && uv pip install --python /tmp/barq-py/bin/python maturin pytest
#        ./scripts/test_python_binding.sh [venv-python]
set -euo pipefail
cd "$(dirname "$0")/../crates/bindings/memory-bindings-python"

PY="${1:-/tmp/barq-py/bin/python}"
export VIRTUAL_ENV="$(dirname "$(dirname "$PY")")"

echo "== maturin develop --release =="
"$VIRTUAL_ENV/bin/maturin" develop --release

echo "== pytest =="
cat > /tmp/test_agent_memory.py <<'PY'
import tempfile, os
from agent_memory import Memory

mem = Memory()
saved = mem.remember("Customer prefers email.", user_id="123", tenant_id="acme")
assert saved["text"] == "Customer prefers email."

hits = mem.recall("How should I contact this customer?", user_id="123", limit=5)
assert len(hits) >= 1 and "email" in hits[0]["text"]

dbdir = tempfile.mkdtemp()
db = os.path.join(dbdir, "mem.redb")
m1 = Memory(db, namespace="smoke")
fact = m1.remember("Project Atlas uses PostgreSQL", memory_type="semantic")
del m1

m2 = Memory(db, namespace="smoke")
found = m2.search("atlas postgresql")
assert any(h["id"] == fact["id"] for h in found)

newer = m2.update(fact["id"], "Atlas migrated to MySQL")
chain = m2.history(newer["id"])
assert len(chain) == 2

assert m2.forget(newer["id"]) is True
assert not [h for h in m2.search("atlas mysql") if h["id"] == newer["id"]]

print("PYTHON BINDING SMOKE TEST OK")
PY
"$PY" /tmp/test_agent_memory.py
