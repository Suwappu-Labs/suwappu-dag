# gsx-dag root composition.
#
# Skeleton only — no resources defined yet. Real validator-node provisioning
# lands in DAG-S20 (full node E2E) per docs/architecture/sprint-map.md.
#
# When validator-node provisioning lands, it will compose as:
#
# module "validators" {
#   source         = "./modules/validator-node"
#   count          = var.validator_count
#   environment    = var.environment
#   instance_type  = var.instance_type
#   region         = var.region
# }

data "aws_caller_identity" "current" {}

output "account_id" {
  description = "AWS account ID validated against expected gsn account."
  value       = data.aws_caller_identity.current.account_id
}

output "expected_account_id" {
  description = "Expected gsn account ID. Mismatch means wrong AWS profile is configured."
  value       = "492042618949"
}
