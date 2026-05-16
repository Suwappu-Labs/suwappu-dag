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
