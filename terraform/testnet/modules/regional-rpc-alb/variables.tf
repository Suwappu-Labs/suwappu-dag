# Inputs for one region's RPC ALB. Instantiated once per seed region by
# terraform/testnet/main.tf, behind a single Global Accelerator (ga.tf).
# The ALB lives in the validator's own VPC so it can target the instance
# in-region (sidesteps the ALB target_type=ip cross-VPC/public-IP limit
# that left the old single-region target group empty → 503).

variable "name_prefix" {
  description = "Resource name prefix, ending in a hyphen (e.g. gsx-testnet-)."
  type        = string
}

variable "region_label" {
  description = "Region label for resource names/tags (e.g. us-east-1)."
  type        = string
}

variable "vpc_id" {
  description = "Validator VPC id (module.<region>.vpc_id). The ALB is placed here."
  type        = string
}

variable "subnet_ids" {
  description = "Two+ public subnet ids in distinct AZs (module.<region>.alb_subnet_ids; requires with_alb_subnets=true on the validator module)."
  type        = list(string)

  validation {
    condition     = length(var.subnet_ids) >= 2
    error_message = "An ALB needs subnets in >=2 AZs; set with_alb_subnets=true on the validator module."
  }
}

variable "target_instance_id" {
  description = "Validator EC2 instance id to register as the ALB target (module.<region>.instance_id)."
  type        = string
}

variable "target_port" {
  description = "Validator JSON-RPC/WebSocket port (var.rpc_port, 9092)."
  type        = number
}

variable "certificate_arn" {
  description = "ARN of a validated ACM certificate IN THIS REGION covering rpc/ws.testnet.* (from rpc_certs.tf). The ALB HTTPS listener terminates TLS with it."
  type        = string
}

variable "waf_rate_limit" {
  description = "Per-IP request rate limit (5-min window) for the REGIONAL WAF ACL."
  type        = number
  default     = 10000
}
