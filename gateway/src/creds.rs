//! Credentials.
//!
//! Two layers:
//! 1. **Agent credentials** – bearer tokens identifying an agent. Each token is bound to
//!    a fixed list of tool scopes (checked again by Cedar via `principal.scopes`).
//! 2. **Scoped call credentials** – after Cedar allows a call, the gateway mints a
//!    short-lived HMAC-signed credential bound to *exactly* that agent + tool + target.
//!    Tool executors refuse to run without a credential matching the operation, so a
//!    tool can never be reached by a path that skipped authorization, and a credential
//!    for `read_file docs/a.md` is useless for `read_file secrets/x.env`.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::AgentConfig;

type HmacSha256 = Hmac<Sha256>;

/// Looks up agents by bearer token (tokens are stored only as SHA-256 hashes).
pub struct AgentRegistry {
    by_hash: HashMap<[u8; 32], AgentConfig>,
}

impl AgentRegistry {
    pub fn new(agents: &[AgentConfig]) -> Result<Self> {
        let mut by_hash = HashMap::new();
        for a in agents {
            let token = a
                .resolved_token()
                .with_context(|| format!("agent {} has no token configured", a.id))?;
            by_hash.insert(Sha256::digest(token.as_bytes()).into(), a.clone());
        }
        Ok(Self { by_hash })
    }

    pub fn authenticate(&self, bearer: &str) -> Option<&AgentConfig> {
        let h: [u8; 32] = Sha256::digest(bearer.as_bytes()).into();
        self.by_hash.get(&h)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scope {
    pub agent: String,
    pub tool: String,
    pub target: String,
    pub exp: i64,
    pub nonce: String,
}

pub struct CredentialIssuer {
    key: Vec<u8>,
    ttl_secs: i64,
}

impl CredentialIssuer {
    pub fn new(key: Vec<u8>, ttl_secs: i64) -> Self {
        Self { key, ttl_secs }
    }

    /// Uses `GATEWAY_SIGNING_KEY` if set, otherwise a random per-process key.
    pub fn from_env() -> Self {
        let key = std::env::var("GATEWAY_SIGNING_KEY")
            .map(String::into_bytes)
            .unwrap_or_else(|_| rand::random::<[u8; 32]>().to_vec());
        Self::new(key, 30)
    }

    fn sign(&self, payload: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("hmac accepts any key size");
        mac.update(payload.as_bytes());
        B64.encode(mac.finalize().into_bytes())
    }

    pub fn issue(&self, agent: &str, tool: &str, target: &str) -> String {
        let scope = Scope {
            agent: agent.into(),
            tool: tool.into(),
            target: target.into(),
            exp: chrono::Utc::now().timestamp() + self.ttl_secs,
            nonce: B64.encode(rand::random::<[u8; 12]>()),
        };
        let payload = B64.encode(serde_json::to_vec(&scope).expect("scope serializes"));
        let sig = self.sign(&payload);
        format!("{payload}.{sig}")
    }

    /// Verifies signature, expiry and that the credential covers this exact operation.
    pub fn verify(&self, token: &str, tool: &str, target: &str) -> Result<Scope> {
        let (payload, sig) = token.split_once('.').context("malformed credential")?;
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("hmac accepts any key size");
        mac.update(payload.as_bytes());
        mac.verify_slice(&B64.decode(sig)?)
            .map_err(|_| anyhow::anyhow!("bad signature"))?;

        let scope: Scope = serde_json::from_slice(&B64.decode(payload)?)?;
        if scope.exp < chrono::Utc::now().timestamp() {
            bail!("credential expired");
        }
        if scope.tool != tool || scope.target != target {
            bail!(
                "credential scope {}:{} does not cover {tool}:{target}",
                scope.tool,
                scope.target
            );
        }
        Ok(scope)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_is_bound_to_operation() {
        let issuer = CredentialIssuer::new(b"k".to_vec(), 30);
        let c = issuer.issue("a", "read_file", "docs/a.md");
        assert!(issuer.verify(&c, "read_file", "docs/a.md").is_ok());
        assert!(issuer.verify(&c, "read_file", "secrets/x.env").is_err());
        assert!(issuer.verify(&c, "delete_file", "docs/a.md").is_err());
    }

    #[test]
    fn tampered_or_expired_credentials_fail() {
        let issuer = CredentialIssuer::new(b"k".to_vec(), -1);
        let c = issuer.issue("a", "read_file", "x");
        assert!(issuer
            .verify(&c, "read_file", "x")
            .unwrap_err()
            .to_string()
            .contains("expired"));

        let other = CredentialIssuer::new(b"different".to_vec(), 30);
        assert!(other.verify(&c, "read_file", "x").is_err());
    }
}
