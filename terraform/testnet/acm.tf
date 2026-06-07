# ACM certificate for the testnet subdomain.
#
# Wildcard cert: `*.testnet.suwappu.globalsettlement.com` covers
# rpc / ws / faucet / explorer / status / program. Same shape as
# terraform/devnet/acm.tf.

resource "aws_acm_certificate" "wildcard" {
  provider    = aws.us_east_1
  domain_name = "*.${var.testnet_subdomain}.${var.apex_domain}"
  subject_alternative_names = [
    "${var.testnet_subdomain}.${var.apex_domain}",
  ]
  validation_method = "DNS"

  tags = {
    Name = "suwappu-testnet-wildcard"
  }

  lifecycle {
    create_before_destroy = true
  }
}

resource "aws_route53_record" "wildcard_validation" {
  for_each = {
    for opt in aws_acm_certificate.wildcard.domain_validation_options :
    opt.domain_name => {
      name   = opt.resource_record_name
      record = opt.resource_record_value
      type   = opt.resource_record_type
    }
  }
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = each.value.name
  type     = each.value.type
  records  = [each.value.record]
  ttl      = 60
}

resource "aws_acm_certificate_validation" "wildcard" {
  provider                = aws.us_east_1
  certificate_arn         = aws_acm_certificate.wildcard.arn
  validation_record_fqdns = [for r in aws_route53_record.wildcard_validation : r.fqdn]
}

output "wildcard_cert_arn" {
  description = "ACM certificate ARN covering *.testnet.suwappu.globalsettlement.com."
  value       = aws_acm_certificate_validation.wildcard.certificate_arn
}
