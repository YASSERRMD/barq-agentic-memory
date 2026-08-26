"""HTTP client mirroring the Rust/TypeScript/.NET SDKs concept-for-concept."""
from __future__ import annotations

import json
import urllib.error
import urllib.request
from dataclasses import dataclass
from typing import Any, Optional


class BarqError(Exception):
    """API-level failure (status, message)."""

    def __init__(self, status: int, message: str) -> None:
        super().__init__(f"api ({status}): {message}")
        self.status = status
        self.message = message


@dataclass
class MemoryView:
    id: str
    type: str
    text: str
    status: str
    version: int
    confidence: float

    @staticmethod
    def from_json(d: dict[str, Any]) -> "MemoryView":
        return MemoryView(
            id=d["id"],
            type=d.get("type", "semantic"),
            text=d.get("text", ""),
            status=d.get("status", "active"),
            version=int(d.get("version", 0)),
            confidence=float(d.get("confidence", 0.5)),
        )


class Memory:
    """Client for one Barq memory server."""

    def __init__(self, base_url: str, timeout: float = 10.0) -> None:
        self.base = base_url.rstrip("/")
        self.timeout = timeout

    def _call(self, method: str, path: str, body: Optional[dict] = None) -> Any:
        url = f"{self.base}{path}"
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(
            url, data=data, method=method,
            headers={"content-type": "application/json"} if data else {},
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                raw = resp.read()
                return json.loads(raw) if raw else None
        except urllib.error.HTTPError as e:
            raw = e.read()
            try:
                envelope = json.loads(raw)
                message = envelope.get("message", str(e))
            except Exception:
                message = str(e)
            raise BarqError(e.code, message) from None

    # --- six concepts, same names everywhere ---------------------------

    def remember(self, text: str, *, tenant_id: Optional[str] = None,
                 user_id: Optional[str] = None,
                 memory_type: Optional[str] = None,
                 confidence: Optional[float] = None) -> MemoryView:
        body: dict[str, Any] = {"text": text}
        if tenant_id: body["tenant_id"] = tenant_id
        if user_id: body["user_id"] = user_id
        if memory_type: body["type"] = memory_type
        if confidence is not None: body["confidence"] = confidence
        return MemoryView.from_json(self._call("POST", "/v1/memories", body))

    def get(self, memory_id: str) -> Optional[MemoryView]:
        try:
            return MemoryView.from_json(self._call("GET", f"/v1/memories/{memory_id}"))
        except BarqError as e:
            if e.status == 404:
                return None
            raise

    def recall(self, query: str, *, tenant_id: Optional[str] = None,
               limit: int = 10) -> list[MemoryView]:
        body: dict[str, Any] = {"query": query, "limit": limit}
        if tenant_id: body["tenant_id"] = tenant_id
        return [MemoryView.from_json(h) for h in self._call("POST", "/v1/recall", body)]

    def search(self, query: str, *, tenant_id: Optional[str] = None,
               limit: int = 10) -> list[MemoryView]:
        body: dict[str, Any] = {"query": query, "limit": limit}
        if tenant_id: body["tenant_id"] = tenant_id
        return [MemoryView.from_json(h) for h in self._call("POST", "/v1/search", body)]

    def update(self, memory_id: str, new_text: str) -> MemoryView:
        return MemoryView.from_json(
            self._call("PATCH", f"/v1/memories/{memory_id}", {"text": new_text}))

    def forget(self, memory_id: str, *, hard: bool = False) -> None:
        suffix = "?hard=true" if hard else ""
        self._call("DELETE", f"/v1/memories/{memory_id}{suffix}")

    def history(self, memory_id: str) -> list[MemoryView]:
        return [MemoryView.from_json(r)
                for r in self._call("GET", f"/v1/memories/{memory_id}/history")]
