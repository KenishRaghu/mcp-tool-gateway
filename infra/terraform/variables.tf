variable "region" {
  type    = string
  default = "us-east-1"
}

variable "environment" {
  description = "staging | production"
  type        = string
}

variable "owner" {
  type    = string
  default = "agent-platform"
}

variable "image" {
  description = "Gateway container image. Production requires a digest pin (repo@sha256:...)."
  type        = string
}

variable "gateway_mode" {
  description = "enforce | audit. Sentinel requires enforce in production."
  type        = string
  default     = "enforce"
}

variable "vpc_id" {
  type = string
}

variable "private_subnet_ids" {
  type = list(string)
}

variable "allowed_ingress_cidrs" {
  description = "CIDRs of agent runtimes allowed to call the gateway."
  type        = list(string)
}

variable "assign_public_ip" {
  type    = bool
  default = false
}

variable "otlp_endpoint" {
  description = "OpenTelemetry collector endpoint for gateway traces."
  type        = string
  default     = ""
}

variable "desired_count" {
  type    = number
  default = 2
}

variable "extra_env" {
  description = "Extra non-secret environment variables for the gateway container."
  type        = map(string)
  default     = {}
}

variable "signing_key_secret_arn" {
  description = "Secrets Manager ARN of the scoped-credential signing key."
  type        = string
}

variable "hermes_token_secret_arn" {
  description = "Secrets Manager ARN of the Hermes agent bearer token."
  type        = string
}
