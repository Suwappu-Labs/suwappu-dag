variable "region_label" {
  description = "Human-readable region label baked into the validator config (matches NodeConfig::self_id)."
  type        = string
}

variable "authority_id" {
  description = "0-indexed Authority Ring id. Matches the validator's genesis-manifest index."
  type        = number
}

variable "instance_type" {
  description = "EC2 instance type."
  type        = string
}

variable "ssh_public_key" {
  description = "OpenSSH-formatted public key for operator access."
  type        = string
}

variable "operator_ip_cidr" {
  description = "CIDR allowed SSH access."
  type        = string
}

variable "consensus_port" {
  description = "Validator peer TCP listen port."
  type        = number
}

variable "client_port" {
  description = "Client intent submission port."
  type        = number
}

variable "artifact_bucket" {
  description = "S3 bucket holding the gsx-node artifact + genesis manifest."
  type        = string
}
