# Perf testnet inputs.

variable "instance_type" {
  description = "EC2 instance type. t3.small is sufficient for the 100 tps perf run."
  type        = string
  default     = "t3.small"
}

variable "operator_ip_cidrs" {
  description = "CIDRs allowed SSH access. Pass each operator IP as a /32 — do not include 0.0.0.0/0. Multiple entries let the same operator SSH from different networks (e.g. home + mobile hotspot) without locking the previous one out on every re-apply."
  type        = list(string)
}

variable "ssh_public_key" {
  description = "OpenSSH-formatted public key seeded into the instance via cloud-init."
  type        = string
}

variable "artifact_bucket" {
  description = "S3 bucket holding gsx-node musl binary + genesis manifest. Created in this stack."
  type        = string
  default     = "gsx-dag-perf-artifacts"
}

variable "consensus_port" {
  description = "Validator peer TCP listen port. Matches NodeConfig::listen."
  type        = number
  default     = 9090
}

variable "client_port" {
  description = "Client intent submission port. Matches NodeConfig::client_listen."
  type        = number
  default     = 9091
}

variable "network_id" {
  description = "Cluster identifier baked into the genesis manifest."
  type        = string
  default     = "gsx-perf-7r"
}
