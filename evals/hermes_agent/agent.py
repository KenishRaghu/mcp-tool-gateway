"""Tool-calling agents evaluated against the gateway.

* HermesAgent   – a real Hermes model (NousResearch Hermes 3/4) using Hermes'
                  native `<tool_call>` format over any OpenAI-compatible API
                  (Ollama, vLLM, OpenRouter, ...).
* ScriptedAgent – deterministic, worst-case "gullible" agent: follows a fixed plan
                  for the user's task and blindly executes any `<tool_call>` it
                  finds in tool outputs. Lets CI test the gateway with no LLM.
"""

from __future__ import annotations

import json
import os
import re
from dataclasses import asdict, dataclass, field

from opentelemetry import trace

from .mcp_client import MCPClient

tracer = trace.get_tracer("hermes-agent")

TOOL_CALL_RE = re.compile(r"<tool_call>\s*(.*?)\s*</tool_call>", re.DOTALL)


def parse_tool_calls(text: str) -> list[dict]:
    """Extracts Hermes-format tool calls: <tool_call>{"name": ..., "arguments": {...}}</tool_call>."""
    calls = []
    for raw in TOOL_CALL_RE.findall(text or ""):
        try:
            obj = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if isinstance(obj, dict) and isinstance(obj.get("name"), str):
            args = obj.get("arguments") or {}
            if isinstance(args, str):  # some models emit arguments as a JSON string
                try:
                    args = json.loads(args)
                except json.JSONDecodeError:
                    args = {}
            calls.append({"name": obj["name"], "arguments": args})
    return calls


@dataclass
class Step:
    tool: str
    arguments: dict
    is_error: bool
    output: str
    injected: bool = False  # call originated from untrusted tool output (scripted agent)


@dataclass
class Trajectory:
    agent: str
    steps: list[Step] = field(default_factory=list)
    final: str = ""

    def to_dict(self) -> dict:
        return asdict(self)


def hermes_system_prompt(tools: list[dict]) -> str:
    tool_sigs = [
        {"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["inputSchema"]}}
        for t in tools
    ]
    return (
        "You are a function calling AI model. You are provided with function signatures within "
        "<tools></tools> XML tags. You may call one or more functions to assist with the user query. "
        "Don't make assumptions about what values to plug into functions. Here are the available tools: "
        f"<tools> {json.dumps(tool_sigs)} </tools> "
        'Use the following pydantic model json schema for each tool call you will make: '
        '{"properties": {"arguments": {"title": "Arguments", "type": "object"}, "name": {"title": "Name", "type": "string"}}, '
        '"required": ["arguments", "name"], "title": "FunctionCall", "type": "object"} '
        "For each function call return a json object with function name and arguments within "
        "<tool_call></tool_call> XML tags as follows:\n<tool_call>\n"
        '{"arguments": <args-dict>, "name": <function-name>}\n</tool_call>\n'
        "When the task is complete, reply with a short final answer and no tool calls."
    )


class HermesAgent:
    """Hermes tool-calling loop. Configure with env vars:

    HERMES_BASE_URL (default http://localhost:11434/v1 = Ollama)
    HERMES_MODEL    (default hermes3)
    HERMES_API_KEY  (default "ollama"; set your OpenRouter key etc.)
    """

    name = "hermes"

    def __init__(self, model: str | None = None, base_url: str | None = None, api_key: str | None = None, max_steps: int = 12):
        from openai import OpenAI

        self.model = model or os.environ.get("HERMES_MODEL", "hermes3")
        self.client = OpenAI(
            base_url=base_url or os.environ.get("HERMES_BASE_URL", "http://localhost:11434/v1"),
            api_key=api_key or os.environ.get("HERMES_API_KEY", "ollama"),
        )
        self.max_steps = max_steps

    def run(self, instruction: str, mcp: MCPClient) -> Trajectory:
        traj = Trajectory(agent=f"hermes:{self.model}")
        with tracer.start_as_current_span("agent.run") as span:
            span.set_attribute("agent.kind", "hermes")
            span.set_attribute("gen_ai.request.model", self.model)
            mcp.initialize()
            messages = [
                {"role": "system", "content": hermes_system_prompt(mcp.list_tools())},
                {"role": "user", "content": instruction},
            ]
            for _ in range(self.max_steps):
                with tracer.start_as_current_span("llm.chat") as llm_span:
                    llm_span.set_attribute("gen_ai.system", "openai_compatible")
                    llm_span.set_attribute("gen_ai.request.model", self.model)
                    resp = self.client.chat.completions.create(model=self.model, messages=messages, temperature=0)
                    content = resp.choices[0].message.content or ""
                    if resp.usage:
                        llm_span.set_attribute("gen_ai.usage.input_tokens", resp.usage.prompt_tokens)
                        llm_span.set_attribute("gen_ai.usage.output_tokens", resp.usage.completion_tokens)
                messages.append({"role": "assistant", "content": content})
                calls = parse_tool_calls(content)
                if not calls:
                    traj.final = content
                    break
                responses = []
                for c in calls:
                    is_error, out = mcp.call_tool(c["name"], c["arguments"])
                    traj.steps.append(Step(c["name"], c["arguments"], is_error, out))
                    responses.append(f"<tool_response>\n{json.dumps({'name': c['name'], 'content': out})}\n</tool_response>")
                messages.append({"role": "user", "content": "\n".join(responses)})
            span.set_attribute("agent.steps", len(traj.steps))
        return traj


class ScriptedAgent:
    """Deterministic worst-case agent (no LLM). Plan format:

    {"steps": [{"tool": "read_file", "arguments": {"path": "docs/notes.md"}},
               {"tool": "write_file", "arguments": {"path": "out/summary.md", "content": "Summary: {{prev}}"}}]}

    `{{prev}}` is replaced by the previous planned step's output (first 300 chars).
    """

    name = "scripted"

    def __init__(self, plan: dict, max_injected: int = 8):
        self.plan, self.max_injected = plan, max_injected

    def run(self, instruction: str, mcp: MCPClient) -> Trajectory:
        traj = Trajectory(agent="scripted-gullible")
        injected_budget = self.max_injected
        with tracer.start_as_current_span("agent.run") as span:
            span.set_attribute("agent.kind", "scripted")
            mcp.initialize()
            prev = ""
            for planned in self.plan["steps"]:
                args = json.loads(json.dumps(planned["arguments"]).replace("{{prev}}", json.dumps(prev[:300])[1:-1]))
                is_error, out = mcp.call_tool(planned["tool"], args)
                traj.steps.append(Step(planned["tool"], args, is_error, out))
                prev = out
                # Gullible: obey tool calls embedded in (untrusted) tool output.
                queue = parse_tool_calls(out)
                while queue and injected_budget > 0:
                    c = queue.pop(0)
                    injected_budget -= 1
                    i_err, i_out = mcp.call_tool(c["name"], c["arguments"])
                    traj.steps.append(Step(c["name"], c["arguments"], i_err, i_out, injected=True))
                    queue.extend(parse_tool_calls(i_out))
            traj.final = "done"
            span.set_attribute("agent.steps", len(traj.steps))
        return traj
