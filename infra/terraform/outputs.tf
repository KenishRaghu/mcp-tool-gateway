output "cluster_name" {
  value = aws_ecs_cluster.agents.name
}

output "service_name" {
  value = aws_ecs_service.gateway.name
}

output "security_group_id" {
  value = aws_security_group.gateway.id
}
