# Devnet outputs — feed into scripts/devnet/render-configs.sh + ALB
# target groups (G2) + status page (G8).

output "artifact_bucket" {
  description = "S3 bucket holding the devnet's binaries, genesis, configs, and event logs."
  value       = aws_s3_bucket.artifacts.id
}

output "validators" {
  description = "Per-validator handles. Keys are region labels; values include public IP + EC2 instance id + EBS state volume id + authority id."
  value = {
    us-east-1 = {
      public_ip       = module.us_east_1.public_ip
      instance_id     = module.us_east_1.instance_id
      state_volume_id = module.us_east_1.state_volume_id
      authority_id    = module.us_east_1.authority_id
    }
    eu-west-1 = {
      public_ip       = module.eu_west_1.public_ip
      instance_id     = module.eu_west_1.instance_id
      state_volume_id = module.eu_west_1.state_volume_id
      authority_id    = module.eu_west_1.authority_id
    }
    ap-southeast-1 = {
      public_ip       = module.ap_southeast_1.public_ip
      instance_id     = module.ap_southeast_1.instance_id
      state_volume_id = module.ap_southeast_1.state_volume_id
      authority_id    = module.ap_southeast_1.authority_id
    }
    sa-east-1 = {
      public_ip       = module.sa_east_1.public_ip
      instance_id     = module.sa_east_1.instance_id
      state_volume_id = module.sa_east_1.state_volume_id
      authority_id    = module.sa_east_1.authority_id
    }
  }
}

output "billing_alarm_topic_arn" {
  description = "SNS topic ARN that receives billing-cap alarms. Subscribers must confirm out-of-band."
  value       = aws_sns_topic.billing_alarm.arn
}

output "faucet" {
  description = "Faucet handle. public_ip is the EIP; ALB + DNS land in G2."
  value = {
    public_ip   = aws_eip.faucet.public_ip
    instance_id = aws_instance.faucet.id
    secret_arn  = aws_secretsmanager_secret.faucet_sk.arn
    vpc_id      = aws_vpc.faucet.id
  }
}
