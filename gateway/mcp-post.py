#!/usr/bin/env python3
"""In-container MCP helper (installed as /usr/local/bin/mcp-post).

  mcp-post --ensure-running           start the gateway if needed and wait for /healthz
  mcp-post --envelope <base64-json>   POST {"body": ..., "headers": ...} (used by the Harbor agent)
  mcp-post <tool> '<json-args>'       tools/call shortcut (used by oracle solutions)
"""

import base64
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

BASE = os.environ.get("MCP_BASE_URL", "http://127.0.0.1:8080")
TOKEN = os.environ.get("AGENT_TOKEN_HERMES", "dev-hermes-token")


def healthy() -> bool:
    try:
        urllib.request.urlopen(f"{BASE}/healthz", timeout=1)
        return True
    except OSError:
        return False


def ensure_running() -> None:
    if healthy():
        return
    os.makedirs("/logs/agent", exist_ok=True)
    log = open("/logs/agent/gateway.log", "ab")
    subprocess.Popen(["mcp-gateway"], stdout=log, stderr=log, start_new_session=True)
    for _ in range(50):
        if healthy():
            return
        time.sleep(0.2)
    sys.exit("gateway did not become healthy")


def post(body: dict, headers: dict) -> str:
    req = urllib.request.Request(
        f"{BASE}/mcp", data=json.dumps(body).encode(), method="POST",
        headers={"content-type": "application/json", **headers},
    )
    try:
        with urllib.request.urlopen(req, timeout=60) as r:
            return r.read().decode()
    except urllib.error.HTTPError as e:
        return e.read().decode()


def main() -> None:
    args = sys.argv[1:]
    if args == ["--ensure-running"]:
        ensure_running()
    elif len(args) == 2 and args[0] == "--envelope":
        env = json.loads(base64.b64decode(args[1]))
        print(post(env["body"], env["headers"]))
    elif len(args) == 2:
        ensure_running()
        body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": args[0], "arguments": json.loads(args[1])}}
        print(post(body, {"authorization": f"Bearer {TOKEN}"}))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
