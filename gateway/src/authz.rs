//! Cedar authorization for every tool call.
//!
//! * Principal: `Agent::"<id>"` (member of `Role::"<role>"`, carries `scopes`)
//! * Action:    `Action::"<tool name>"`
//! * Resource:  `Tool::"<tool name>"`
//! * Context:   normalized call target (`path`, `domain`, `recipient_domain`)

use std::{collections::HashMap, collections::HashSet, fs, path::Path, str::FromStr};

use anyhow::{anyhow, bail, Context as _, Result};
use cedar_policy::{
    Authorizer as CedarAuthorizer, Context, Decision as CedarDecision, Entities, Entity, EntityId,
    EntityTypeName, EntityUid, PolicySet, Request, RestrictedExpression, Schema, ValidationMode,
    Validator,
};
use serde::Serialize;

use crate::config::AgentConfig;

/// Normalized facts about a tool call that policies can reason over.
#[derive(Debug, Clone, Default, Serialize)]
pub struct CallContext {
    pub tool: String,
    pub path: String,
    pub domain: String,
    pub recipient_domain: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Decision {
    pub allowed: bool,
    /// `@id` annotations of the policies that determined the decision.
    pub policies: Vec<String>,
    pub errors: Vec<String>,
}

pub struct Authorizer {
    policies: PolicySet,
    schema: Schema,
    entities: Entities,
    cedar: CedarAuthorizer,
}

fn uid(ty: &str, id: &str) -> Result<EntityUid> {
    Ok(EntityUid::from_type_name_and_id(
        EntityTypeName::from_str(ty).map_err(|e| anyhow!("{e}"))?,
        EntityId::new(id),
    ))
}

impl Authorizer {
    /// Loads `*.cedar` policies and `gateway.cedarschema` from `policy_dir`,
    /// validates the policies against the schema and builds agent entities.
    pub fn load(policy_dir: &Path, agents: &[AgentConfig]) -> Result<Self> {
        let schema_src = fs::read_to_string(policy_dir.join("gateway.cedarschema"))
            .context("reading gateway.cedarschema")?;
        let (schema, _warnings) =
            Schema::from_cedarschema_str(&schema_src).map_err(|e| anyhow!("schema: {e}"))?;

        let mut src = String::new();
        let mut files: Vec<_> = fs::read_dir(policy_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "cedar"))
            .collect();
        files.sort();
        if files.is_empty() {
            bail!("no .cedar policy files in {}", policy_dir.display());
        }
        for f in files {
            src.push_str(&fs::read_to_string(&f)?);
            src.push('\n');
        }
        let policies = PolicySet::from_str(&src).map_err(|e| anyhow!("policies: {e}"))?;

        let validation = Validator::new(schema.clone()).validate(&policies, ValidationMode::Strict);
        if !validation.validation_passed() {
            let errs: Vec<String> = validation
                .validation_errors()
                .map(|e| e.to_string())
                .collect();
            bail!("policy validation failed: {}", errs.join("; "));
        }

        Self::with_parts(policies, schema, agents)
    }

    fn with_parts(policies: PolicySet, schema: Schema, agents: &[AgentConfig]) -> Result<Self> {
        let mut entities = Vec::new();
        let mut roles = HashSet::new();
        for a in agents {
            let role = uid("Role", &a.role)?;
            roles.insert(a.role.clone());
            let scopes = RestrictedExpression::new_set(
                a.scopes
                    .iter()
                    .map(|s| RestrictedExpression::new_string(s.clone())),
            );
            entities.push(Entity::new(
                uid("Agent", &a.id)?,
                HashMap::from([("scopes".to_string(), scopes)]),
                HashSet::from([role]),
            )?);
        }
        for r in roles {
            entities.push(Entity::new_no_attrs(uid("Role", &r)?, HashSet::new()));
        }
        for t in crate::tools::TOOL_NAMES {
            entities.push(Entity::new_no_attrs(uid("Tool", t)?, HashSet::new()));
        }
        let entities = Entities::from_entities(entities, Some(&schema))?;
        Ok(Self {
            policies,
            schema,
            entities,
            cedar: CedarAuthorizer::new(),
        })
    }

    pub fn authorize(&self, agent_id: &str, ctx: &CallContext) -> Result<Decision> {
        let action = uid("Action", &ctx.tool)?;
        let context = Context::from_pairs(
            [
                ("tool", &ctx.tool),
                ("path", &ctx.path),
                ("domain", &ctx.domain),
                ("recipient_domain", &ctx.recipient_domain),
            ]
            .map(|(k, v)| (k.to_string(), RestrictedExpression::new_string(v.clone()))),
        )?;
        let request = Request::new(
            uid("Agent", agent_id)?,
            action,
            uid("Tool", &ctx.tool)?,
            context,
            Some(&self.schema),
        )
        .map_err(|e| anyhow!("invalid request: {e}"))?;

        let resp = self
            .cedar
            .is_authorized(&request, &self.policies, &self.entities);
        let policies = resp
            .diagnostics()
            .reason()
            .map(|pid| {
                self.policies
                    .policy(pid)
                    .and_then(|p| p.annotation("id").map(str::to_string))
                    .unwrap_or_else(|| pid.to_string())
            })
            .collect();
        let errors = resp.diagnostics().errors().map(|e| e.to_string()).collect();
        Ok(Decision {
            allowed: resp.decision() == CedarDecision::Allow,
            policies,
            errors,
        })
    }
}
