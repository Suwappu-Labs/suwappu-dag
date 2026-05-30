output "alb_arn" {
  description = "ALB ARN — registered as a Global Accelerator endpoint (ga.tf)."
  value       = aws_lb.rpc.arn
}

output "alb_dns_name" {
  description = "ALB DNS name (debugging / direct regional reach)."
  value       = aws_lb.rpc.dns_name
}

output "target_group_arn" {
  description = "RPC target group ARN."
  value       = aws_lb_target_group.rpc.arn
}
