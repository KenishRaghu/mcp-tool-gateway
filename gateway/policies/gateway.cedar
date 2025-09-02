// ---------------------------------------------------------------------------
// MCP tool gateway policies. Default is DENY: a call runs only if some permit
// matches and no forbid matches. `@id` shows up in audit logs and traces.
// ---------------------------------------------------------------------------

// ---- Credential scoping ----------------------------------------------------

@id("credential-scope")
forbid (principal, action, resource)
unless { principal.scopes.contains(context.tool) };

// ---- Assistant role: least privilege --------------------------------------

@id("assistant-read-workspace")
permit (
  principal in Role::"assistant",
  action in [Action::"list_files", Action::"read_file"],
  resource
);

@id("assistant-write-output")
permit (principal in Role::"assistant", action == Action::"write_file", resource)
when { context.path like "out/*" };

@id("assistant-cleanup-tmp")
permit (principal in Role::"assistant", action == Action::"delete_file", resource)
when { context.path like "tmp/*" };

@id("assistant-fetch-allowlist")
permit (principal in Role::"assistant", action == Action::"fetch_url", resource)
when { ["docs.company.internal", "api.github.com"].contains(context.domain) };

@id("assistant-email-internal")
permit (principal in Role::"assistant", action == Action::"send_email", resource)
when { context.recipient_domain == "company.internal" };

// ---- Operator role ---------------------------------------------------------

@id("operator-all-tools")
permit (principal in Role::"operator", action, resource);

// ---- Guardrails that apply to everyone (forbid always wins) ----------------

@id("no-secrets")
forbid (principal, action, resource)
when {
  context.path == "secrets" ||
  context.path like "secrets/*" ||
  context.path like "*.env" ||
  context.path like "*.pem"
};

@id("no-gateway-internals")
forbid (principal, action, resource)
when { context.path == ".sandbox" || context.path like ".sandbox/*" };
