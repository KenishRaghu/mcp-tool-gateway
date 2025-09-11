"""OpenTelemetry setup for eval runs.

Spans are exported over OTLP/HTTP when OTEL_EXPORTER_OTLP_ENDPOINT is set
(e.g. http://localhost:4318 for the bundled Jaeger). Trace context is
propagated to the gateway, so one trace shows:

  eval.task -> agent.run -> llm.chat / mcp.tools/call -> (gateway) mcp.request -> authz.cedar -> tool.execute
"""

from __future__ import annotations

import os

from opentelemetry import trace
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor

_provider: TracerProvider | None = None


def setup_tracing(service_name: str = "hermes-agent-evals") -> TracerProvider:
    global _provider
    if _provider is not None:
        return _provider
    _provider = TracerProvider(resource=Resource.create({"service.name": service_name}))
    if os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT"):
        from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter

        _provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
    trace.set_tracer_provider(_provider)
    return _provider


def shutdown_tracing() -> None:
    if _provider is not None:
        _provider.shutdown()
