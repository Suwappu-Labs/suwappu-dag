output "public_ip" {
  description = "Validator's public EIP. Used to render peer addresses in node.toml + as an ALB target."
  value       = aws_eip.this.public_ip
}

output "instance_id" {
  description = "EC2 instance id. Used by SSM Session Manager + maintenance windows."
  value       = aws_instance.validator.id
}

output "state_volume_id" {
  description = "Persistent EBS volume id holding /var/lib/gsx. Survives instance replacement; carries consensus state + event log."
  value       = aws_ebs_volume.state.id
}

output "region_label" {
  description = "Echoes the region label for downstream consumers."
  value       = var.region_label
}

output "authority_id" {
  description = "Echoes the authority id for downstream consumers."
  value       = var.authority_id
}

output "vpc_id" {
  description = "Validator VPC id. An in-region ALB (testnet RPC fronting) is placed here so it can target the validator instance without cross-VPC peering."
  value       = aws_vpc.this.id
}

output "alb_subnet_ids" {
  description = "Public subnet ids for an in-region ALB. Two AZs when with_alb_subnets=true; a single-element list otherwise (ALB needs >=2, so consumers must set with_alb_subnets)."
  value       = concat([aws_subnet.public.id], aws_subnet.public_b[*].id)
}

output "security_group_id" {
  description = "Validator security group id. RPC port is already open to 0.0.0.0/0, so an in-VPC ALB can reach the instance on rpc_port."
  value       = aws_security_group.this.id
}

output "private_ip" {
  description = "Validator private IP within its VPC (for ip-type ALB targeting; instance-type targeting uses instance_id)."
  value       = aws_instance.validator.private_ip
}
