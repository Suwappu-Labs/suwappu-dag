# G2 — ACM certificate for the devnet subdomain.
#
# Single wildcard cert: `*.devnet.suwappu.globalsettlement.com` covers
# rpc / ws / faucet / explorer / status. CloudFront REQUIRES certs
# in us-east-1 regardless of the distribution's edge regions, so
# the cert lives there even though the ALB itself is also there;
# explorer + status (G7/G8) reuse the same cert ARN.
#
# DNS validation — Route53 zone for the subdomain lives in this
# stack (dns.tf), so terraform can wire the validation records
# fully automatically.

resource "aws_acm_certificate" "wildcard" {
  provider    = aws.us_east_1
  domain_name = "*.${var.devnet_subdomain}.${var.apex_domain}"
  subject_alternative_names = [
    # Apex-of-subdomain (e.g. `devnet.suwappu.globalsettlement.com`)
    # so we can host a landing page there pointing devs at the
    # other subdomains. Saves issuing a second cert.
    "${var.devnet_subdomain}.${var.apex_domain}",
  ]
  validation_method = "DNS"

  tags = {
    Name = "suwappu-devnet-wildcard"
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
  zone_id  = aws_route53_zone.devnet.zone_id
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
  description = "ACM certificate ARN covering *.devnet.suwappu.globalsettlement.com. Consumed by alb.tf + G7 explorer.tf + G8 status.tf."
  value       = aws_acm_certificate_validation.wildcard.certificate_arn
}
