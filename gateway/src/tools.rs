//! Sandboxed tools exposed through the gateway.
//!
//! File tools operate inside the configured workspace directory. Side-effecting
//! "network" tools are sandboxed so evals are deterministic and safe:
//! `send_email` appends to `.sandbox/outbox.jsonl` and `fetch_url` records to
//! `.sandbox/egress.jsonl` (returning a fixture from `.sandbox/web/<domain>.txt`).

use std::{
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{authz::CallContext, creds::CredentialIssuer};

pub const TOOL_NAMES: [&str; 6] = [
    "list_files",
    "read_file",
    "write_file",
    "delete_file",
    "fetch_url",
    "send_email",
];

/// MCP `tools/list` definitions.
pub fn definitions() -> Vec<Value> {
    let s = |props: Value, req: &[&str]| json!({"type": "object", "properties": props, "required": req});
    vec![
        json!({"name": "list_files", "description": "List files in a workspace directory.",
               "inputSchema": s(json!({"dir": {"type": "string", "description": "Directory relative to the workspace, '.' for root"}}), &["dir"])}),
        json!({"name": "read_file", "description": "Read a text file from the workspace.",
               "inputSchema": s(json!({"path": {"type": "string"}}), &["path"])}),
        json!({"name": "write_file", "description": "Write a text file in the workspace.",
               "inputSchema": s(json!({"path": {"type": "string"}, "content": {"type": "string"}}), &["path", "content"])}),
        json!({"name": "delete_file", "description": "Delete a file from the workspace.",
               "inputSchema": s(json!({"path": {"type": "string"}}), &["path"])}),
        json!({"name": "fetch_url", "description": "HTTP GET a URL.",
               "inputSchema": s(json!({"url": {"type": "string"}}), &["url"])}),
        json!({"name": "send_email", "description": "Send an email to a single recipient.",
               "inputSchema": s(json!({"to": {"type": "string"}, "subject": {"type": "string"}, "body": {"type": "string"}}), &["to", "subject", "body"])}),
    ]
}

fn arg<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .with_context(|| format!("missing string argument '{key}'"))
}

/// Normalizes a workspace-relative path; rejects absolute paths and `..` so
/// policies like `context.path like "secrets/*"` cannot be bypassed.
pub fn normalize_path(raw: &str) -> Result<String> {
    let mut parts = Vec::new();
    for c in Path::new(raw.trim()).components() {
        match c {
            Component::Normal(p) => parts.push(p.to_string_lossy().to_string()),
            Component::CurDir => {}
            _ => bail!("path '{raw}' must be relative and must not contain '..'"),
        }
    }
    Ok(if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    })
}

fn url_domain(url: &str) -> Result<String> {
    let rest = url
        .split_once("://")
        .map(|(_, r)| r)
        .context("url must include a scheme")?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit('@').next().unwrap_or_default(); // strip userinfo
    let host = host
        .split(':')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if host.is_empty() {
        bail!("url has no host");
    }
    Ok(host)
}

/// Extracts the facts Cedar needs and the credential target for a call.
pub fn call_context(tool: &str, args: &Value) -> Result<(CallContext, String)> {
    let mut ctx = CallContext {
        tool: tool.into(),
        ..Default::default()
    };
    let target = match tool {
        "list_files" => {
            ctx.path = normalize_path(arg(args, "dir")?)?;
            ctx.path.clone()
        }
        "read_file" | "write_file" | "delete_file" => {
            ctx.path = normalize_path(arg(args, "path")?)?;
            ctx.path.clone()
        }
        "fetch_url" => {
            let url = arg(args, "url")?;
            ctx.domain = url_domain(url)?;
            url.to_string()
        }
        "send_email" => {
            let to = arg(args, "to")?.trim().to_ascii_lowercase();
            let (_, domain) = to.rsplit_once('@').context("invalid email address")?;
            if to.contains([',', ';', ' ']) {
                bail!("exactly one recipient is allowed");
            }
            ctx.recipient_domain = domain.to_string();
            to
        }
        other => bail!("unknown tool '{other}'"),
    };
    Ok((ctx, target))
}

fn append_jsonl(path: PathBuf, v: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(f, "{v}")?;
    Ok(())
}

/// Executes a tool. Requires a scoped credential minted for this exact operation.
pub fn execute(
    workspace: &Path,
    issuer: &CredentialIssuer,
    credential: &str,
    tool: &str,
    target: &str,
    args: &Value,
) -> Result<String> {
    let scope = issuer
        .verify(credential, tool, target)
        .context("scoped credential rejected")?;
    let file = || workspace.join(target);
    match tool {
        "list_files" => {
            let mut names: Vec<String> = fs::read_dir(file())?
                .filter_map(|e| e.ok())
                .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                .map(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    if e.path().is_dir() {
                        format!("{n}/")
                    } else {
                        n
                    }
                })
                .collect();
            names.sort();
            Ok(names.join("\n"))
        }
        "read_file" => Ok(fs::read_to_string(file()).with_context(|| format!("reading {target}"))?),
        "write_file" => {
            let content = arg(args, "content")?;
            if let Some(dir) = file().parent() {
                fs::create_dir_all(dir)?;
            }
            fs::write(file(), content)?;
            Ok(format!("wrote {} bytes to {target}", content.len()))
        }
        "delete_file" => {
            fs::remove_file(file()).with_context(|| format!("deleting {target}"))?;
            Ok(format!("deleted {target}"))
        }
        "fetch_url" => {
            let domain = url_domain(target)?;
            append_jsonl(
                workspace.join(".sandbox/egress.jsonl"),
                &json!({"agent": scope.agent, "url": target}),
            )?;
            let fixture = workspace.join(".sandbox/web").join(format!("{domain}.txt"));
            Ok(fs::read_to_string(fixture).unwrap_or_else(|_| "200 OK".into()))
        }
        "send_email" => {
            append_jsonl(
                workspace.join(".sandbox/outbox.jsonl"),
                &json!({"agent": scope.agent, "to": target, "subject": arg(args, "subject")?, "body": arg(args, "body")?}),
            )?;
            Ok(format!("email queued to {target}"))
        }
        other => bail!("unknown tool '{other}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_rejects_traversal() {
        assert_eq!(normalize_path("./docs//a.md").unwrap(), "docs/a.md");
        assert_eq!(normalize_path(".").unwrap(), ".");
        assert!(normalize_path("docs/../secrets/k.env").is_err());
        assert!(normalize_path("/etc/passwd").is_err());
    }

    #[test]
    fn extracts_domains() {
        assert_eq!(
            url_domain("https://User@Evil.example:8443/x?y").unwrap(),
            "evil.example"
        );
        let (ctx, _) = call_context(
            "send_email",
            &json!({"to": "Bob@Company.Internal", "subject": "", "body": ""}),
        )
        .unwrap();
        assert_eq!(ctx.recipient_domain, "company.internal");
        assert!(call_context(
            "send_email",
            &json!({"to": "a@x.com, b@y.com", "subject": "", "body": ""})
        )
        .is_err());
    }
}
