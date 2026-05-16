# Testnet inputs.

variable "instance_type" {
  description = "EC2 instance type for each seed validator. Testnet runs heavier than devnet because external operators benchmark against it — c7g.xlarge (arm64, 4 vCPU, 8 GB) absorbs the higher TPS target without saturating. ~$96/mo per instance on-demand; 7×$96 ≈ $672/mo for the seed set."
  type        = string
  default     = "c7g.xlarge"
}

variable "operator_ip_cidrs" {
  description = "CIDRs allowed SSH access. Pass each operator IP as a /32 — do not include 0.0.0.0/0. Same posture as devnet — operator SSH is a fallback; SSM Session Manager is primary."
  type        = list(string)
}

variable "ssh_public_key" {
  description = "OpenSSH-formatted public key seeded into the instance via cloud-init."
  type        = string
}

variable "artifact_bucket" {
  description = "S3 bucket holding gsx-node musl binary + genesis manifest. Created in this stack. Separate from devnet + perf so a testnet binary rollout doesn't touch other environments."
  type        = string
  default     = "gsx-dag-testnet-artifacts"
}

variable "external_uploads_bucket" {
  description = "S3 bucket where external validator operators upload their events.ndjson snapshots for the points program. Public-WRITE (with per-IAM-role scoping) so operators don't need foundation-side credentials; public-READ blocked."
  type        = string
  default     = "gsx-dag-testnet-validator-uploads"
}

variable "consensus_port" {
  description = "Validator peer TCP listen port."
  type        = number
  default     = 9090
}

variable "client_port" {
  description = "Client intent submission port."
  type        = number
  default     = 9091
}

variable "rpc_port" {
  description = "JSON-RPC HTTP port."
  type        = number
  default     = 9092
}

variable "metrics_port" {
  description = "Prometheus /metrics scrape port. Bound to 127.0.0.1 only; CloudWatch agent scrapes locally."
  type        = number
  default     = 9093
}

variable "network_id" {
  description = "Cluster identifier baked into the genesis manifest. SDKs hard-code this to reject responses from the wrong network."
  type        = string
  default     = "gsx-testnet-v1"
}

variable "chain_id" {
  description = "Numeric chain identifier. Testnet = 20251 (year-of-launch + suffix). Mainnet will be 1; devnet is 2025. Distinct so signed transactions don't replay across environments."
  type        = number
  default     = 20251
}

variable "state_volume_gb" {
  description = "Per-validator persistent EBS volume size (gp3). Testnet runs longer + higher TPS than devnet — 200 GB covers ~12 months of event log + state at the projected 5k-TPS target."
  type        = number
  default     = 200
}

variable "monthly_billing_cap_usd" {
  description = "CloudWatch billing alarm threshold. Testnet is more expensive than devnet (7 nodes, larger instances, validator-program EC2, more bandwidth) — budget $2000/mo."
  type        = number
  default     = 2000
}

variable "billing_alarm_email" {
  description = "Email subscriber for the cost-cap SNS topic. Required."
  type        = string
}

variable "testnet_subdomain" {
  description = "Subdomain under globalsettlement.com for testnet endpoints. Mirror of devnet.gsx.globalsettlement.com but on testnet.gsx.globalsettlement.com."
  type        = string
  default     = "testnet.gsx"
}

variable "apex_domain" {
  description = "Apex domain that hosts the testnet subdomain."
  type        = string
  default     = "globalsettlement.com"
}
