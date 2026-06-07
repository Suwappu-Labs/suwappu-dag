# Devnet inputs.

variable "instance_type" {
  description = "EC2 instance type for each validator. t4g.medium (arm64, 2 vCPU, 4 GB) is the cost/perf sweet spot for an always-on 4-region devnet. ~$24/mo per instance on-demand."
  type        = string
  default     = "t4g.medium"
}

variable "operator_ip_cidrs" {
  description = "CIDRs allowed SSH access. Pass each operator IP as a /32 — do not include 0.0.0.0/0. Same posture as perf — operator console is one of many access paths; SSM Session Manager is the primary."
  type        = list(string)
}

variable "ssh_public_key" {
  description = "OpenSSH-formatted public key seeded into the instance via cloud-init."
  type        = string
}

variable "artifact_bucket" {
  description = "S3 bucket holding suwappu-node musl binary + genesis manifest. Created in this stack. Separate bucket from perf so the two environments can't accidentally pull each other's genesis."
  type        = string
  default     = "suwappu-dag-devnet-artifacts"
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

variable "rpc_port" {
  description = "JSON-RPC HTTP port. Matches NodeConfig::rpc_listen. Open to the world on the security group; per-IP rate limit applies at the application layer."
  type        = number
  default     = 9092
}

variable "metrics_port" {
  description = "Prometheus /metrics scrape port. Bound to 127.0.0.1 only; CloudWatch agent on the same instance scrapes it. NOT exposed externally."
  type        = number
  default     = 9093
}

variable "network_id" {
  description = "Cluster identifier baked into the genesis manifest. SDKs hard-code this so they reject responses from the wrong network."
  type        = string
  default     = "suwappu-devnet"
}

variable "chain_id" {
  description = "Numeric chain identifier. Devnet = 2025 (year-of-launch as a memorable tag). Mainnet will be 1."
  type        = number
  default     = 2025
}

variable "state_volume_gb" {
  description = "Per-validator persistent EBS volume size (gp3) mounted at /var/lib/suwappu. Survives instance replacement so consensus state + events.ndjson aren't lost when an instance is rebuilt. 50 GB covers ~6 months of event log at projected devnet TPS."
  type        = number
  default     = 50
}

variable "monthly_billing_cap_usd" {
  description = "CloudWatch billing alarm threshold. Devnet is cost-capped at this monthly burn; alarm fires SNS notification when projected/actual spend exceeds this."
  type        = number
  default     = 500
}

variable "billing_alarm_email" {
  description = "Email subscriber for the cost-cap SNS topic. Required — without an alarm subscriber, the cap is non-functional."
  type        = string
}
