"""Minimal MCP client (Streamable HTTP, JSON responses).

The transport is pluggable so the same agent runs:
  * locally  -> HttpTransport (urllib to http://127.0.0.1:<port>/mcp)
  * in Harbor -> a transport that runs `curl` inside the task container
"""

from __future__ import annotations

import itertools
import json
import urllib.error
import urllib.request
from typing import Callable, Protocol

from opentelemetry import propagate, trace

tracer = trace.get_tracer("hermes-agent")


class Transport(Protocol):
    def __call__(self, body: dict, headers: dict[str, str]) -> dict: ...


class HttpTransport:
    def __init__(self, url: str, timeout: float = 30):
        self.url, self.timeout = url, timeout

    def __call__(self, body: dict, headers: dict[str, str]) -> dict:
        req = urllib.request.Request(
            self.url,
            data=json.dumps(body).encode(),
            headers={"content-type": "application/json", **headers},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                raw = resp.read()
        except urllib.error.HTTPError as e:
            raw = e.read()
        return json.loads(raw) if raw else {}


class MCPClient:
    def __init__(self, transport: Transport | Callable[[dict, dict], dict], token: str):
        self.transport, self.token = transport, token
        self._ids = itertools.count(1)

    def _rpc(self, method: str, params: dict | None = None) -> dict:
        headers = {"authorization": f"Bearer {self.token}"}
        propagate.inject(headers)  # W3C traceparent -> gateway joins the same trace
        body = {"jsonrpc": "2.0", "id": next(self._ids), "method": method, "params": params or {}}
        resp = self.transport(body, headers)
        if "error" in resp:
            raise RuntimeError(f"MCP {method} failed: {resp['error']}")
        return resp.get("result", {})

    def initialize(self) -> dict:
        return self._rpc(
            "initialize",
            {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "hermes-agent", "version": "0.1"}},
        )

    def list_tools(self) -> list[dict]:
        return self._rpc("tools/list")["tools"]

    def call_tool(self, name: str, arguments: dict) -> tuple[bool, str]:
        """Returns (is_error, text)."""
        with tracer.start_as_current_span(f"mcp.tools/call {name}") as span:
            span.set_attribute("mcp.tool.name", name)
            result = self._rpc("tools/call", {"name": name, "arguments": arguments})
            is_error = bool(result.get("isError"))
            text = "\n".join(c.get("text", "") for c in result.get("content", []))
            span.set_attribute("mcp.tool.is_error", is_error)
            span.set_attribute("mcp.tool.denied", text.startswith("Denied:"))
            return is_error, text
