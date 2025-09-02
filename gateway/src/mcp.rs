//! MCP server over Streamable HTTP (JSON responses): `POST /mcp` with JSON-RPC 2.0.
//!
//! Every `tools/call` goes through: authenticate agent -> normalize target ->
//! Cedar authorization -> mint scoped credential -> execute -> audit.

use std::sync::Arc;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use opentelemetry::trace::TraceContextExt;
use serde_json::{json, Value};
use tracing::{field::Empty, info_span, Instrument};
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::{
    audit::{AuditLog, AuditRecord},
    authz::Authorizer,
    config::{AgentConfig, Config, Mode},
    creds::{AgentRegistry, CredentialIssuer},
    telemetry, tools,
};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub struct AppState {
    pub config: Config,
    pub authz: Authorizer,
    pub agents: AgentRegistry,
    pub issuer: CredentialIssuer,
    pub audit: AuditLog,
}

impl AppState {
    pub fn new(config: Config, issuer: CredentialIssuer) -> anyhow::Result<Self> {
        Ok(Self {
            authz: Authorizer::load(&config.policy_dir, &config.agents)?,
            agents: AgentRegistry::new(&config.agents)?,
            audit: AuditLog::new(config.audit_log.clone()),
            issuer,
            config,
        })
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/mcp", post(handle))
        .route("/healthz", get(|| async { "ok" }))
        .with_state(state)
}

fn rpc_result(id: &Value, result: Value) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

fn rpc_error(status: StatusCode, id: &Value, code: i64, msg: &str) -> Response {
    (
        status,
        Json(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})),
    )
        .into_response()
}

fn tool_result(text: impl Into<String>, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text.into()}], "isError": is_error})
}

async fn handle(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> Response {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let Some(agent) = bearer.and_then(|t| st.agents.authenticate(t)).cloned() else {
        return rpc_error(
            StatusCode::UNAUTHORIZED,
            &id,
            -32001,
            "missing or invalid agent credential",
        );
    };

    let span =
        info_span!("mcp.request", rpc.method = %method, agent.id = %agent.id, otel.kind = "server");
    if let Err(e) = span.set_parent(telemetry::parent_context(&headers)) {
        tracing::warn!(error = ?e, "could not attach parent trace context");
    }

    async move {
        match method.as_str() {
            "initialize" => rpc_result(
                &id,
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "mcp-gateway", "version": env!("CARGO_PKG_VERSION")}
                }),
            ),
            m if m.starts_with("notifications/") => StatusCode::ACCEPTED.into_response(),
            "ping" => rpc_result(&id, json!({})),
            "tools/list" => {
                // Only advertise tools the agent's credential is scoped to.
                let tools: Vec<Value> = tools::definitions()
                    .into_iter()
                    .filter(|t| agent.scopes.iter().any(|s| t["name"] == s.as_str()))
                    .collect();
                rpc_result(&id, json!({"tools": tools}))
            }
            "tools/call" => {
                let params = req.get("params").cloned().unwrap_or_default();
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                rpc_result(&id, call_tool(&st, &agent, name, &args))
            }
            _ => rpc_error(StatusCode::OK, &id, -32601, "method not found"),
        }
    }
    .instrument(span)
    .await
}

fn current_trace_id() -> String {
    tracing::Span::current()
        .context()
        .span()
        .span_context()
        .trace_id()
        .to_string()
}

fn call_tool(st: &AppState, agent: &AgentConfig, tool: &str, args: &Value) -> Value {
    let span =
        info_span!("tool.call", tool.name = %tool, authz.decision = Empty, authz.policies = Empty);
    let _enter = span.enter();
    let trace_id = current_trace_id();
    let audit = |decision: &str, policies: &[String], outcome: &str| {
        st.audit.write(&AuditRecord {
            ts: chrono::Utc::now().to_rfc3339(),
            trace_id: trace_id.clone(),
            agent: &agent.id,
            tool,
            args,
            decision,
            policies,
            outcome,
        })
    };

    // 1. Normalize the call into facts Cedar can reason over.
    let (ctx, target) = match tools::call_context(tool, args) {
        Ok(v) => v,
        Err(e) => {
            audit("deny", &[], "invalid-arguments");
            return tool_result(format!("Invalid tool call: {e}"), true);
        }
    };

    // 2. Cedar authorization.
    let decision = match info_span!("authz.cedar").in_scope(|| st.authz.authorize(&agent.id, &ctx))
    {
        Ok(d) => d,
        Err(e) => {
            audit("deny", &[], "authz-error");
            return tool_result(format!("Authorization error: {e}"), true);
        }
    };
    span.record(
        "authz.decision",
        if decision.allowed { "allow" } else { "deny" },
    );
    span.record("authz.policies", decision.policies.join(",").as_str());

    if !decision.allowed {
        let why = if decision.policies.is_empty() {
            "no permit policy matched (default deny)".to_string()
        } else {
            format!("forbidden by policy {}", decision.policies.join(", "))
        };
        tracing::warn!(agent = %agent.id, tool, target = %target, %why, "tool call denied");
        if st.config.mode == Mode::Enforce {
            audit("deny", &decision.policies, "blocked");
            return tool_result(format!("Denied: {tool}({target}) {why}."), true);
        }
    }

    // 3. Mint a credential scoped to exactly this operation, then execute.
    let credential = st.issuer.issue(&agent.id, tool, &target);
    let result = info_span!("tool.execute").in_scope(|| {
        tools::execute(
            &st.config.workspace,
            &st.issuer,
            &credential,
            tool,
            &target,
            args,
        )
    });
    let decision_label = if decision.allowed {
        "allow"
    } else {
        "deny-audit-only"
    };
    match result {
        Ok(out) => {
            audit(decision_label, &decision.policies, "ok");
            tool_result(out, false)
        }
        Err(e) => {
            audit(decision_label, &decision.policies, "error");
            tool_result(format!("Tool error: {e:#}"), true)
        }
    }
}
