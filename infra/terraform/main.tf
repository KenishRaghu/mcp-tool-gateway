# Agent infrastructure: the MCP tool gateway on ECS Fargate.
# Every change is gated by Sentinel (../sentinel) before rollout.

locals {
  name = "mcp-gateway-${var.environment}"
  tags = {
    owner               = var.owner
    environment         = var.environment
    data-classification = "confidential"
    service             = "mcp-gateway"
  }
}

# Upstream secrets (scoped-credential signing key, agent tokens) are provisioned
# in Secrets Manager by the security team and injected at runtime. Agents only
# ever hold their own bearer token; tools receive per-call scoped credentials.

resource "aws_cloudwatch_log_group" "gateway" {
  name              = "/ecs/${local.name}"
  retention_in_days = 30
  tags              = local.tags
}

resource "aws_ecs_cluster" "agents" {
  name = "${local.name}-cluster"
  setting {
    name  = "containerInsights"
    value = "enabled"
  }
  tags = local.tags
}

data "aws_iam_policy_document" "ecs_assume" {
  statement {
    actions = ["sts:AssumeRole"]
    principals {
      type        = "Service"
      identifiers = ["ecs-tasks.amazonaws.com"]
    }
  }
}

resource "aws_iam_role" "execution" {
  name               = "${local.name}-execution"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume.json
  tags               = local.tags
}

resource "aws_iam_role_policy_attachment" "execution" {
  role       = aws_iam_role.execution.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy"
}

data "aws_iam_policy_document" "read_secrets" {
  statement {
    actions   = ["secretsmanager:GetSecretValue"]
    resources = [var.signing_key_secret_arn, var.hermes_token_secret_arn]
  }
}

resource "aws_iam_role_policy" "execution_secrets" {
  name   = "read-gateway-secrets"
  role   = aws_iam_role.execution.id
  policy = data.aws_iam_policy_document.read_secrets.json
}

# Task role: the gateway itself needs no AWS permissions.
resource "aws_iam_role" "task" {
  name               = "${local.name}-task"
  assume_role_policy = data.aws_iam_policy_document.ecs_assume.json
  tags               = local.tags
}

resource "aws_security_group" "gateway" {
  name        = "${local.name}-sg"
  description = "MCP gateway: only agent runtimes may connect"
  vpc_id      = var.vpc_id
  tags        = local.tags

  ingress {
    description = "MCP from agent runtimes"
    from_port   = 8080
    to_port     = 8080
    protocol    = "tcp"
    cidr_blocks = var.allowed_ingress_cidrs
  }

  egress {
    description = "HTTPS (OTLP, AWS APIs)"
    from_port   = 443
    to_port     = 443
    protocol    = "tcp"
    cidr_blocks = ["0.0.0.0/0"]
  }
}

resource "aws_ecs_task_definition" "gateway" {
  family                   = local.name
  requires_compatibilities = ["FARGATE"]
  network_mode             = "awsvpc"
  cpu                      = 256
  memory                   = 512
  execution_role_arn       = aws_iam_role.execution.arn
  task_role_arn            = aws_iam_role.task.arn
  tags                     = local.tags

  container_definitions = jsonencode([{
    name         = "mcp-gateway"
    image        = var.image
    essential    = true
    privileged   = false
    portMappings = [{ containerPort = 8080, protocol = "tcp" }]
    environment = concat([
      { name = "GATEWAY_MODE", value = var.gateway_mode },
      { name = "GATEWAY_POLICY_DIR", value = "/etc/mcp-gateway/policies" },
      { name = "GATEWAY_LISTEN", value = "0.0.0.0:8080" },
      { name = "OTEL_EXPORTER_OTLP_ENDPOINT", value = var.otlp_endpoint },
      { name = "OTEL_SERVICE_NAME", value = "mcp-gateway" },
    ], [for k, v in var.extra_env : { name = k, value = v }])
    secrets = [
      { name = "GATEWAY_SIGNING_KEY", valueFrom = var.signing_key_secret_arn },
      { name = "AGENT_TOKEN_HERMES", valueFrom = var.hermes_token_secret_arn },
    ]
    logConfiguration = {
      logDriver = "awslogs"
      options = {
        awslogs-group         = aws_cloudwatch_log_group.gateway.name
        awslogs-region        = var.region
        awslogs-stream-prefix = "gateway"
      }
    }
  }])
}

resource "aws_ecs_service" "gateway" {
  name            = local.name
  cluster         = aws_ecs_cluster.agents.id
  task_definition = aws_ecs_task_definition.gateway.arn
  desired_count   = var.desired_count
  launch_type     = "FARGATE"
  tags            = local.tags

  network_configuration {
    subnets          = var.private_subnet_ids
    security_groups  = [aws_security_group.gateway.id]
    assign_public_ip = var.assign_public_ip
  }
}
