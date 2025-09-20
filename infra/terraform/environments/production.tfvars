# Compliant production rollout (passes the Sentinel gate).
environment             = "production"
image                   = "123456789012.dkr.ecr.us-east-1.amazonaws.com/mcp-gateway@sha256:3f1c5a9b2e8d7c6f4a0b1e2d3c4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b9c0d1e2f"
gateway_mode            = "enforce"
vpc_id                  = "vpc-0abc1234def567890"
private_subnet_ids      = ["subnet-0aaa1111bbb22222c", "subnet-0ddd3333eee44444f"]
allowed_ingress_cidrs   = ["10.20.0.0/16"]
otlp_endpoint           = "http://otel-collector.observability.internal:4318"
signing_key_secret_arn  = "arn:aws:secretsmanager:us-east-1:123456789012:secret:mcp-gateway/signing-key-AbCdEf"
hermes_token_secret_arn = "arn:aws:secretsmanager:us-east-1:123456789012:secret:mcp-gateway/agent-token-hermes-GhIjKl"
