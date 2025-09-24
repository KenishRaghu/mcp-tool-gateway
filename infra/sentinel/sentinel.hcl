# Sentinel gate for rolling out the MCP gateway (agent infrastructure).
#   hard-mandatory: blocks rollout, cannot be overridden
#   soft-mandatory: blocks rollout unless explicitly overridden
#   advisory:       warning only

import "module" "common" {
  source = "./common-functions.sentinel"
}

policy "enforce-cedar-mode" {
  source            = "./enforce-cedar-mode.sentinel"
  enforcement_level = "hard-mandatory"
}

policy "pinned-image-digest" {
  source            = "./pinned-image-digest.sentinel"
  enforcement_level = "hard-mandatory"
}

policy "no-public-exposure" {
  source            = "./no-public-exposure.sentinel"
  enforcement_level = "hard-mandatory"
}

policy "no-plaintext-secrets" {
  source            = "./no-plaintext-secrets.sentinel"
  enforcement_level = "hard-mandatory"
}

policy "required-tags" {
  source            = "./required-tags.sentinel"
  enforcement_level = "soft-mandatory"
}

policy "otel-tracing-enabled" {
  source            = "./otel-tracing-enabled.sentinel"
  enforcement_level = "advisory"
}
