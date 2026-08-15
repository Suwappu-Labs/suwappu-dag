output "artifact_bucket" {
  description = "S3 bucket holding the testnet's binaries, genesis, configs, event logs."
  value       = aws_s3_bucket.artifacts.id
}

output "external_uploads_bucket" {
  description = "S3 bucket where external validator operators upload their rotated events.ndjson for the points program."
  value       = aws_s3_bucket.external_uploads.id
}

output "validators" {
  description = "Per-seed-validator handles. Keys are region labels; values include public IP + EC2 instance id + EBS state volume id + authority id."
  value = {
    us-east-1 = {
      public_ip       = module.us_east_1.public_ip
      instance_id     = module.us_east_1.instance_id
      state_volume_id = module.us_east_1.state_volume_id
      authority_id    = module.us_east_1.authority_id
    }
    us-west-2 = {
      public_ip       = module.us_west_2.public_ip
      instance_id     = module.us_west_2.instance_id
      state_volume_id = module.us_west_2.state_volume_id
      authority_id    = module.us_west_2.authority_id
    }
    eu-west-1 = {
      public_ip       = module.eu_west_1.public_ip
      instance_id     = module.eu_west_1.instance_id
      state_volume_id = module.eu_west_1.state_volume_id
      authority_id    = module.eu_west_1.authority_id
    }
    eu-central-1 = {
      public_ip       = module.eu_central_1.public_ip
      instance_id     = module.eu_central_1.instance_id
      state_volume_id = module.eu_central_1.state_volume_id
      authority_id    = module.eu_central_1.authority_id
    }
    ap-southeast-1 = {
      public_ip       = module.ap_southeast_1.public_ip
      instance_id     = module.ap_southeast_1.instance_id
      state_volume_id = module.ap_southeast_1.state_volume_id
      authority_id    = module.ap_southeast_1.authority_id
    }
    ap-northeast-1 = {
      public_ip       = module.ap_northeast_1.public_ip
      instance_id     = module.ap_northeast_1.instance_id
      state_volume_id = module.ap_northeast_1.state_volume_id
      authority_id    = module.ap_northeast_1.authority_id
    }
    sa-east-1 = {
      public_ip       = module.sa_east_1.public_ip
      instance_id     = module.sa_east_1.instance_id
      state_volume_id = module.sa_east_1.state_volume_id
      authority_id    = module.sa_east_1.authority_id
    }
  }
}

output "validator_program" {
  description = "Points-accumulator handle. EC2 + RDS where the daemon (follow-up PR) runs."
  value = {
    ec2_public_ip   = aws_eip.program.public_ip
    ec2_instance_id = aws_instance.program.id
    db_endpoint     = aws_db_instance.program.endpoint
    db_secret_arn   = aws_secretsmanager_secret.program_db_password.arn
    s3_uploads      = aws_s3_bucket.external_uploads.id
  }
}

output "billing_alarm_topic_arn" {
  description = "SNS topic ARN that receives billing-cap alarms."
  value       = aws_sns_topic.billing_alarm.arn
}

output "faucet" {
  description = "Faucet handle. public_ip is the EIP behind the ALB; the public URL is faucet.testnet.suwappu.bot."
  value = {
    public_ip   = aws_eip.faucet.public_ip
    instance_id = aws_instance.faucet.id
    secret_arn  = aws_secretsmanager_secret.faucet_sk.arn
    vpc_id      = aws_vpc.faucet.id
  }
}

output "public_urls" {
  description = "Externally-reachable URLs for the testnet. SDKs + dApp examples + docs use these as the canonical reference."
  value = {
    rpc      = "https://rpc.${var.testnet_subdomain}.${var.apex_domain}"
    ws       = "wss://ws.${var.testnet_subdomain}.${var.apex_domain}/ws"
    faucet   = "https://faucet.${var.testnet_subdomain}.${var.apex_domain}"
    program  = "https://program.${var.testnet_subdomain}.${var.apex_domain}"
    explorer = "https://explorer.${var.testnet_subdomain}.${var.apex_domain}"
    status   = "https://status.${var.testnet_subdomain}.${var.apex_domain}"
  }
}
