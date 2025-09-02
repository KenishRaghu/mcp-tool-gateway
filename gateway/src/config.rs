//! Gateway configuration (TOML file + env overrides).

use std::{env, fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Denied calls are blocked (production).
    Enforce,
    /// Denied calls are logged but still executed (dry-run rollout only).
    Audit,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentConfig {
    /// Cedar principal id, e.g. `Agent::"hermes-eval"`.
    pub id: String,
    /// Cedar role the agent belongs to, e.g. `Role::"assistant"`.
    pub role: String,
    /// Bearer token the agent presents. Prefer `token_env` outside of dev.
    #[serde(default)]
    pub token: Option<String>,
    /// Name of an env var holding the bearer token.
    #[serde(default)]
    pub token_env: Option<String>,
    /// Tools this credential is scoped to. Enforced in Cedar via `principal.scopes`.
    pub scopes: Vec<String>,
}

impl AgentConfig {
    pub fn resolved_token(&self) -> Option<String> {
        self.token_env
            .as_ref()
            .and_then(|k| env::var(k).ok())
            .or_else(|| self.token.clone())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub listen: String,
    pub workspace: PathBuf,
    pub policy_dir: PathBuf,
    pub audit_log: PathBuf,
    pub mode: Mode,
    pub agents: Vec<AgentConfig>,
}

impl Config {
    /// Loads the config from `GATEWAY_CONFIG` (default `gateway.toml`) and applies
    /// `GATEWAY_LISTEN`, `GATEWAY_WORKSPACE`, `GATEWAY_AUDIT_LOG`, `GATEWAY_POLICY_DIR`
    /// and `GATEWAY_MODE` overrides.
    pub fn load() -> Result<Self> {
        let path = env::var("GATEWAY_CONFIG").unwrap_or_else(|_| "gateway.toml".into());
        let raw = fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
        let mut cfg: Config = toml::from_str(&raw).with_context(|| format!("parsing {path}"))?;

        if let Ok(v) = env::var("GATEWAY_LISTEN") {
            cfg.listen = v;
        }
        if let Ok(v) = env::var("GATEWAY_WORKSPACE") {
            cfg.workspace = v.into();
        }
        if let Ok(v) = env::var("GATEWAY_AUDIT_LOG") {
            cfg.audit_log = v.into();
        }
        if let Ok(v) = env::var("GATEWAY_POLICY_DIR") {
            cfg.policy_dir = v.into();
        }
        if let Ok(v) = env::var("GATEWAY_MODE") {
            cfg.mode = match v.as_str() {
                "enforce" => Mode::Enforce,
                "audit" => Mode::Audit,
                other => bail!("GATEWAY_MODE must be enforce|audit, got {other}"),
            };
        }
        Ok(cfg)
    }
}
