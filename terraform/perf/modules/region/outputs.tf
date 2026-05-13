output "public_ip" {
  description = "Validator's public EIP. Used to render peer addresses in node.toml."
  value       = aws_eip.this.public_ip
}

output "instance_id" {
  description = "EC2 instance id."
  value       = aws_instance.validator.id
}

output "region_label" {
  description = "Echoes the region label for downstream consumers."
  value       = var.region_label
}

output "authority_id" {
  description = "Echoes the authority id for downstream consumers."
  value       = var.authority_id
}
