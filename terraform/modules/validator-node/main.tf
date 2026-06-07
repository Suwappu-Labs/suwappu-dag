# validator-node module — placeholder.
#
# Lands in DAG-S20 (full node E2E). Will provision:
#
# - EC2 instance (instance type from variables, paper §13.1 reference is
#   c6i.* / Inf2 / G5 class with 100 Gbps NIC + 512 GB RAM + H100-80GB GPU
#   for FRI proving)
# - EBS gp3 volume sized for suwappu-db RocksDB + block store + LTP corridor data
# - Security group: SCION transport ports, gossip ports, RPC ingress
# - IAM role for CloudWatch + Secrets Manager (Authority-Node signing key)
# - User-data bootstrap that pulls a release tag from this repo and starts
#   `suwappu-node` under systemd

variable "environment" {
  description = "Deployment environment."
  type        = string
}

variable "instance_type" {
  description = "EC2 instance type."
  type        = string
}

variable "region" {
  description = "AWS region."
  type        = string
}
