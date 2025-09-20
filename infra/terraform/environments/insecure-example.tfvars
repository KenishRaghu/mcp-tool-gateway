# Deliberately NON-compliant config: shows what the Sentinel gate blocks.
environment             = "production"
owner                   = ""                                   # no owning team
image                   = "ghcr.io/example/mcp-gateway:latest" # mutable tag
gateway_mode            = "audit"                              # Cedar not enforced
vpc_id                  = "vpc-0abc1234def567890"
private_subnet_ids      = ["subnet-0aaa1111bbb22222c"]
allowed_ingress_cidrs   = ["0.0.0.0/0"] # open to the internet
assign_public_ip        = true
otlp_endpoint           = ""                                          # no tracing
extra_env               = { AGENT_TOKEN_HERMES = "dev-hermes-token" } # secret in plaintext env
signing_key_secret_arn  = "arn:aws:secretsmanager:us-east-1:123456789012:secret:mcp-gateway/signing-key-AbCdEf"
hermes_token_secret_arn = "arn:aws:secretsmanager:us-east-1:123456789012:secret:mcp-gateway/agent-token-hermes-GhIjKl"
