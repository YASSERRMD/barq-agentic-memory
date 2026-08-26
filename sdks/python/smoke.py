"""End-to-end Python SDK smoke test against a running server."""
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from barq_memory import Memory

base = os.environ.get("BARQ_BASE", "http://127.0.0.1:18099")
client = Memory(base)

saved = client.remember("Python SDK smoke fact", tenant_id="acme")
assert saved.id and saved.text == "Python SDK smoke fact"

hits = client.recall("sdk smoke fact", tenant_id="acme", limit=5)
assert any(h.id == saved.id for h in hits), "recall failed"

successor = client.update(saved.id, "Python SDK smoke fact v2")
chain = client.history(successor.id)
assert len(chain) == 2, f"history {len(chain)} != 2"

client.forget(successor.id)
assert client.get(successor.id) is None, "forgotten still visible"

print("PYTHON SDK SMOKE TEST OK")
