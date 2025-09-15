"""Run the eval tasks locally (no Docker/Harbor needed).

For every task in evals/tasks/ and every gateway mode:
  1. copy the task workspace to a temp dir
  2. start the Rust gateway on a free port pointed at it
  3. run the agent (scripted or Hermes) through the gateway, traced with OpenTelemetry
  4. score with the task's tests/check.py (same verifier Harbor uses)

Modes:
  enforce – Cedar decisions are enforced (the real deployment)
  audit   – decisions are only logged = "no gateway" baseline, shows what attacks would do

Example:
  python evals/run_local.py --agent scripted --mode both
  HERMES_MODEL=hermes3 python evals/run_local.py --agent hermes
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from opentelemetry import trace

from hermes_agent import HermesAgent, HttpTransport, MCPClient, ScriptedAgent
from hermes_agent.telemetry import setup_tracing, shutdown_tracing

ROOT = Path(__file__).resolve().parent.parent
EVALS = ROOT / "evals"
GATEWAY_DIR = ROOT / "gateway"
TOKEN = os.environ.get("AGENT_TOKEN_HERMES", "dev-hermes-token")


def gateway_bin() -> Path:
    if os.environ.get("GATEWAY_BIN"):
        return Path(os.environ["GATEWAY_BIN"])
    for profile in ("release", "debug"):
        p = GATEWAY_DIR / "target" / profile / "mcp-gateway"
        if p.exists():
            return p
    sys.exit("gateway binary not found: run `cargo build --release` in gateway/ or set GATEWAY_BIN")


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def start_gateway(workspace: Path, audit_log: Path, mode: str, log_file: Path) -> tuple[subprocess.Popen, str]:
    port = free_port()
    env = {
        **os.environ,
        "GATEWAY_CONFIG": str(GATEWAY_DIR / "gateway.toml"),
        "GATEWAY_POLICY_DIR": str(GATEWAY_DIR / "policies"),
        "GATEWAY_WORKSPACE": str(workspace),
        "GATEWAY_AUDIT_LOG": str(audit_log),
        "GATEWAY_LISTEN": f"127.0.0.1:{port}",
        "GATEWAY_MODE": mode,
    }
    proc = subprocess.Popen([str(gateway_bin())], env=env, stdout=log_file.open("w"), stderr=subprocess.STDOUT)
    base = f"http://127.0.0.1:{port}"
    for _ in range(100):
        try:
            urllib.request.urlopen(f"{base}/healthz", timeout=0.5)
            return proc, f"{base}/mcp"
        except OSError:
            if proc.poll() is not None:
                sys.exit(f"gateway exited early, see {log_file}")
            time.sleep(0.1)
    proc.kill()
    sys.exit("gateway did not become healthy")


def make_agent(kind: str, task: str):
    if kind == "hermes":
        return HermesAgent()
    plan = json.loads((EVALS / "scripted_plans" / f"{task}.json").read_text())
    return ScriptedAgent(plan)


def run_task(task_dir: Path, agent_kind: str, mode: str, out_dir: Path) -> dict:
    task = task_dir.name
    tracer = trace.get_tracer("evals")
    out = out_dir / mode / task
    out.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory() as tmp:
        ws = Path(tmp) / "workspace"
        shutil.copytree(task_dir / "environment" / "workspace", ws)
        audit_log = out / "audit.jsonl"
        audit_log.unlink(missing_ok=True)
        proc, url = start_gateway(ws, audit_log, mode, out / "gateway.log")
        try:
            with tracer.start_as_current_span("eval.task") as span:
                span.set_attribute("eval.task", task)
                span.set_attribute("eval.gateway_mode", mode)
                span.set_attribute("eval.agent", agent_kind)
                instruction = (task_dir / "instruction.md").read_text()
                traj = make_agent(agent_kind, task).run(instruction, MCPClient(HttpTransport(url), TOKEN))
                (out / "trajectory.json").write_text(json.dumps(traj.to_dict(), indent=2))
        finally:
            proc.terminate()
            proc.wait(timeout=10)

        check = subprocess.run(
            [sys.executable, "check.py", str(ws), str(audit_log)],
            cwd=task_dir / "tests", capture_output=True, text=True,
        )
    try:
        result = json.loads(check.stdout)
    except json.JSONDecodeError:
        result = {"utility": False, "safe": False, "error": check.stderr[-2000:]}
    result.update(task=task, mode=mode, reward=int(check.returncode == 0), attack=not task.startswith("benign"))
    (out / "result.json").write_text(json.dumps(result, indent=2))
    return result


def summarize(results: list[dict], agent_kind: str) -> str:
    lines = [f"# Eval report – agent: `{agent_kind}`", ""]
    for mode in sorted({r["mode"] for r in results}, reverse=True):
        rs = [r for r in results if r["mode"] == mode]
        attacks = [r for r in rs if r["attack"]]
        label = "Cedar enforced" if mode == "enforce" else "audit-only (no protection baseline)"
        lines += [
            f"## Gateway mode: `{mode}` – {label}",
            "",
            "| task | reward | utility | safe | blocked calls |",
            "|---|---|---|---|---|",
        ]
        lines += [
            f"| {r['task']} | {r['reward']} | {'✅' if r['utility'] else '❌'} | {'✅' if r['safe'] else '❌'} | {r.get('blocked_calls', 0)} |"
            for r in rs
        ]
        asr = sum(not r["safe"] for r in attacks) / max(len(attacks), 1)
        lines += [
            "",
            f"- **Task success (reward):** {sum(r['reward'] for r in rs)}/{len(rs)}",
            f"- **Utility:** {sum(r['utility'] for r in rs)}/{len(rs)}",
            f"- **Attack success rate:** {asr:.0%} ({sum(not r['safe'] for r in attacks)}/{len(attacks)} attack tasks unsafe)",
            "",
        ]
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--agent", choices=["scripted", "hermes"], default="scripted")
    ap.add_argument("--mode", choices=["enforce", "audit", "both"], default="both")
    ap.add_argument("--task", action="append", help="run only these tasks (repeatable)")
    ap.add_argument("--out", type=Path, default=ROOT / "results")
    args = ap.parse_args()

    setup_tracing()
    modes = ["enforce", "audit"] if args.mode == "both" else [args.mode]
    tasks = sorted(p for p in (EVALS / "tasks").iterdir() if (p / "task.toml").exists())
    if args.task:
        tasks = [t for t in tasks if t.name in args.task]

    results = []
    for mode in modes:
        for t in tasks:
            r = run_task(t, args.agent, mode, args.out)
            print(f"[{mode:7}] {t.name:32} reward={r['reward']} utility={r['utility']} safe={r['safe']} blocked={r.get('blocked_calls', 0)}")
            results.append(r)
    shutdown_tracing()

    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "report.json").write_text(json.dumps(results, indent=2))
    report = summarize(results, args.agent)
    (args.out / "report.md").write_text(report)
    print("\n" + report)

    # CI gate: with enforcement on, every task must be safe and useful.
    enforced = [r for r in results if r["mode"] == "enforce"]
    return 0 if all(r["reward"] for r in enforced) else 1


if __name__ == "__main__":
    sys.exit(main())
