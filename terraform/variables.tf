variable "region" {
  description = "AWS region for gsx-dag infrastructure."
  type        = string
  default     = "us-east-1"
}

variable "environment" {
  description = "Deployment environment (dev, testnet, mainnet-beta)."
  type        = string
  default     = "dev"

  validation {
    condition     = contains(["dev", "testnet", "mainnet-beta"], var.environment)
    error_message = "environment must be one of dev, testnet, mainnet-beta."
  }
}

variable "validator_count" {
  description = "Number of validator-node EC2 instances to provision."
  type        = number
  default     = 0
}

variable "instance_type" {
  description = "EC2 instance type for validator nodes. Paper §13.1 reference: 100 Gbps NIC, 512 GB RAM, NVIDIA H100-80GB. Dev default is much smaller."
  type        = string
  default     = "t3.small"
}
