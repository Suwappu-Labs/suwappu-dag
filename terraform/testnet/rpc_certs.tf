# Regional ACM certs for the per-region RPC ALBs (regional_alb.tf).
#
# An ALB terminates TLS with a cert IN ITS OWN REGION. The existing
# us-east-1 wildcard cert (acm.tf) is reused by the us-east-1 ALB; the
# other six regions need their own copies.
#
# ACM returns the SAME DNS-validation CNAME for a given domain across
# every certificate request in one account, so all of these validate
# against the existing `aws_route53_record.wildcard_validation` records
# (acm.tf / dns.tf) — no new Route53 records, no resource conflict.
# (If a future AWS change ever returns per-cert tokens, each validation
# below would need its own records keyed off its own
# domain_validation_options; `terraform plan` + a stuck
# aws_acm_certificate_validation would surface that.)

locals {
  rpc_cert_domain = "*.${var.testnet_subdomain}.${var.apex_domain}"
  rpc_cert_san    = "${var.testnet_subdomain}.${var.apex_domain}"
  # Reused by every regional validation below + by the us-east-1 ALB
  # via aws_acm_certificate_validation.wildcard (acm.tf).
  wildcard_validation_fqdns = [for r in aws_route53_record.wildcard_validation : r.fqdn]
}

# --- us-west-2 ---
resource "aws_acm_certificate" "rpc_us_west_2" {
  provider                  = aws.us_west_2
  domain_name               = local.rpc_cert_domain
  subject_alternative_names = [local.rpc_cert_san]
  validation_method         = "DNS"
  tags                      = { Name = "gsx-testnet-rpc-us-west-2" }
  lifecycle { create_before_destroy = true }
}
resource "aws_acm_certificate_validation" "rpc_us_west_2" {
  provider                = aws.us_west_2
  certificate_arn         = aws_acm_certificate.rpc_us_west_2.arn
  validation_record_fqdns = local.wildcard_validation_fqdns
}

# --- eu-west-1 ---
resource "aws_acm_certificate" "rpc_eu_west_1" {
  provider                  = aws.eu_west_1
  domain_name               = local.rpc_cert_domain
  subject_alternative_names = [local.rpc_cert_san]
  validation_method         = "DNS"
  tags                      = { Name = "gsx-testnet-rpc-eu-west-1" }
  lifecycle { create_before_destroy = true }
}
resource "aws_acm_certificate_validation" "rpc_eu_west_1" {
  provider                = aws.eu_west_1
  certificate_arn         = aws_acm_certificate.rpc_eu_west_1.arn
  validation_record_fqdns = local.wildcard_validation_fqdns
}

# --- eu-central-1 ---
resource "aws_acm_certificate" "rpc_eu_central_1" {
  provider                  = aws.eu_central_1
  domain_name               = local.rpc_cert_domain
  subject_alternative_names = [local.rpc_cert_san]
  validation_method         = "DNS"
  tags                      = { Name = "gsx-testnet-rpc-eu-central-1" }
  lifecycle { create_before_destroy = true }
}
resource "aws_acm_certificate_validation" "rpc_eu_central_1" {
  provider                = aws.eu_central_1
  certificate_arn         = aws_acm_certificate.rpc_eu_central_1.arn
  validation_record_fqdns = local.wildcard_validation_fqdns
}

# --- ap-southeast-1 ---
resource "aws_acm_certificate" "rpc_ap_southeast_1" {
  provider                  = aws.ap_southeast_1
  domain_name               = local.rpc_cert_domain
  subject_alternative_names = [local.rpc_cert_san]
  validation_method         = "DNS"
  tags                      = { Name = "gsx-testnet-rpc-ap-southeast-1" }
  lifecycle { create_before_destroy = true }
}
resource "aws_acm_certificate_validation" "rpc_ap_southeast_1" {
  provider                = aws.ap_southeast_1
  certificate_arn         = aws_acm_certificate.rpc_ap_southeast_1.arn
  validation_record_fqdns = local.wildcard_validation_fqdns
}

# --- ap-northeast-1 ---
resource "aws_acm_certificate" "rpc_ap_northeast_1" {
  provider                  = aws.ap_northeast_1
  domain_name               = local.rpc_cert_domain
  subject_alternative_names = [local.rpc_cert_san]
  validation_method         = "DNS"
  tags                      = { Name = "gsx-testnet-rpc-ap-northeast-1" }
  lifecycle { create_before_destroy = true }
}
resource "aws_acm_certificate_validation" "rpc_ap_northeast_1" {
  provider                = aws.ap_northeast_1
  certificate_arn         = aws_acm_certificate.rpc_ap_northeast_1.arn
  validation_record_fqdns = local.wildcard_validation_fqdns
}

# --- sa-east-1 ---
resource "aws_acm_certificate" "rpc_sa_east_1" {
  provider                  = aws.sa_east_1
  domain_name               = local.rpc_cert_domain
  subject_alternative_names = [local.rpc_cert_san]
  validation_method         = "DNS"
  tags                      = { Name = "gsx-testnet-rpc-sa-east-1" }
  lifecycle { create_before_destroy = true }
}
resource "aws_acm_certificate_validation" "rpc_sa_east_1" {
  provider                = aws.sa_east_1
  certificate_arn         = aws_acm_certificate.rpc_sa_east_1.arn
  validation_record_fqdns = local.wildcard_validation_fqdns
}
