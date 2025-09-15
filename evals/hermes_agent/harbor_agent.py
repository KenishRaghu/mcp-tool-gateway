"""Harbor adapter: runs the Hermes agent against the MCP gateway inside a Harbor task container.

The task image (FROM mcp-gateway:local) contains the gateway and the task workspace.
The agent loop runs on the host (where the LLM endpoint is reachable); each MCP
request is executed *inside* the container (via `mcp-post`), so every tool call hits the
gateway -> Cedar -> sandboxed tools, and the verifier then inspects the container.

Usage (from the repo root, after `docker build -t mcp-gateway:local .`):

  PYTHONPATH=evals harbor run -p evals/tasks \
      --agent hermes_agent.harbor_agent:HermesHarborAgent -m hermes3

  # no LLM: worst-case scripted agent
  PYTHONPATH=evals harbor run -p evals/tasks \
      --agent hermes_agent.harbor_agent:HermesHarborAgent --ak agent_kind=scripted
"""

from __future__ import annotations

import asyncio
import base64
import json
import os
from pathlib import Path

from harbor.agents.base import BaseAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext
from opentelemetry import trace

from .agent import HermesAgent, ScriptedAgent
from .mcp_client import MCPClient
from .telemetry import setup_tracing

EVALS = Path(__file__).resolve().parent.parent


class HermesHarborAgent(BaseAgent):
    def __init__(self, logs_dir: Path, model_name: str | None = None, agent_kind: str = "hermes", **kwargs):
        super().__init__(logs_dir=logs_dir, model_name=model_name, **kwargs)
        self.agent_kind = agent_kind

    @staticmethod
    def name() -> str:
        return "hermes-mcp-gateway"

    def version(self) -> str:
        return "0.1.0"

    async def setup(self, environment: BaseEnvironment) -> None:
        setup_tracing("hermes-harbor-agent")
        # Harbor may override the image CMD, so make sure the gateway is running.
        r = await environment.exec("mcp-post --ensure-running", timeout_sec=30)
        if r.return_code != 0:
            raise RuntimeError(f"gateway did not start inside the task container: {r.stderr}")

    def _make_agent(self, instruction: str):
        if self.agent_kind == "scripted":
            # Pick the plan whose task instruction matches.
            for task_dir in (EVALS / "tasks").iterdir():
                inst = task_dir / "instruction.md"
                if inst.exists() and inst.read_text().strip() == instruction.strip():
                    return ScriptedAgent(json.loads((EVALS / "scripted_plans" / f"{task_dir.name}.json").read_text()))
            raise RuntimeError("no scripted plan for this instruction")
        return HermesAgent(model=self.model_name or os.environ.get("HERMES_MODEL"))

    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        loop = asyncio.get_running_loop()

        def transport(body: dict, headers: dict[str, str]) -> dict:
            envelope = base64.b64encode(json.dumps({"body": body, "headers": headers}).encode()).decode()
            fut = asyncio.run_coroutine_threadsafe(environment.exec(f"mcp-post --envelope {envelope}", timeout_sec=60), loop)
            res = fut.result()
            return json.loads(res.stdout or "{}")

        token = os.environ.get("AGENT_TOKEN_HERMES", "dev-hermes-token")
        agent = self._make_agent(instruction)
        with trace.get_tracer("evals").start_as_current_span("eval.harbor_trial") as span:
            span.set_attribute("eval.agent", self.agent_kind)
            # The agent loop is synchronous; run it in a thread and bridge exec calls back.
            traj = await asyncio.to_thread(agent.run, instruction, MCPClient(transport, token))

        self.logs_dir.mkdir(parents=True, exist_ok=True)
        (self.logs_dir / "trajectory.json").write_text(json.dumps(traj.to_dict(), indent=2))
        context.metadata = {
            "tool_calls": len(traj.steps),
            "denied_calls": sum(s.output.startswith("Denied:") for s in traj.steps),
        }
