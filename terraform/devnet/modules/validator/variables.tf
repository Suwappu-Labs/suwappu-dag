variable "name_prefix" {
  description = "Prefix applied to every named AWS resource the module creates (Name tag, key_pair name, IAM role + instance-profile name, SG name). Must end with a hyphen. Concrete instantiations: devnet passes `gsx-dev-`; testnet passes `gsx-devnet-` (historical — the testnet went live before this var existed and owns the `gsx-devnet-` namespace; renaming to `gsx-testnet-` would force destroy+recreate of every keypair/SG/IAM role and is deferred to the next clean window)."
  type        = string

  validation {
    condition     = can(regex("^[a-z0-9-]+-$", var.name_prefix))
    error_message = "name_prefix must be lowercase alphanumerics + hyphens, ending in a hyphen (e.g. gsx-dev-)."
  }
}

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
  description = "S3 bucket holding the gsx-node artifact + genesis manifest."
  type        = string
}

variable "state_volume_gb" {
  description = "Persistent EBS volume size (gp3) mounted at /var/lib/gsx."
  type        = number
}

variable "with_alb_subnets" {
  description = "When true, create a second public subnet in a different AZ so a co-located in-region ALB (the testnet RPC fronting) can live in this VPC and target the validator without cross-VPC peering — ALBs require subnets in >=2 AZs. Default false: the devnet stack does not front validators with an ALB and stays single-subnet (no change to its plan)."
  type        = bool
  default     = false
}
