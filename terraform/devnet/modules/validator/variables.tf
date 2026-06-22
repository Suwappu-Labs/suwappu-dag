variable "region_label" {
  description = "Human-readable region label baked into the validator config (matches NodeConfig::self_id)."
  type        = string
}

variable "authority_id" {
  description = "0-indexed Authority Ring id. Matches the validator's genesis-manifest index."
  type        = number
}

variable "instance_type" {
  description = "EC2 instance type. Devnet default: t4g.medium (arm64). All validators must use the same architecture so the binary uploaded to S3 runs everywhere."
  type        = string
}

variable "ssh_public_key" {
  description = "OpenSSH-formatted public key for operator access."
  type        = string
}

variable "operator_ip_cidrs" {
  description = "CIDRs allowed SSH access (list of /32s). SSM Session Manager is the primary access path; SSH is a fallback."
  type        = list(string)
}

variable "consensus_port" {
  description = "Validator peer TCP listen port."
  type        = number
}

variable "client_port" {
  description = "Client intent submission port."
  type        = number
}

variable "rpc_port" {
  description = "JSON-RPC HTTP port. Opened to the world; per-IP rate limit applies at the app layer."
  type        = number
}

variable "metrics_port" {
  description = "Prometheus /metrics scrape port. NOT exposed externally — only the local CloudWatch agent reads it."
  type        = number
}

variable "artifact_bucket" {
  description = "S3 bucket holding the suwappu-node artifact + genesis manifest."
  type        = string
}

variable "state_volume_gb" {
  description = "Persistent EBS volume size (gp3) mounted at /var/lib/suwappu."
  type        = number
}
