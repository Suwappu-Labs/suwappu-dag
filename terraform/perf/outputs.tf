# Public outputs — consumed by scripts/perf/render-configs.sh.

output "validators" {
  description = "Map of region_label -> { authority_id, public_ip }. Six regions; af-south-1 is gated behind a one-time AWS account opt-in."
  value = {
    "us-east-1"      = { authority_id = module.us_east_1.authority_id, public_ip = module.us_east_1.public_ip }
    "us-west-2"      = { authority_id = module.us_west_2.authority_id, public_ip = module.us_west_2.public_ip }
    "eu-west-1"      = { authority_id = module.eu_west_1.authority_id, public_ip = module.eu_west_1.public_ip }
    "ap-northeast-1" = { authority_id = module.ap_northeast_1.authority_id, public_ip = module.ap_northeast_1.public_ip }
    "ap-southeast-2" = { authority_id = module.ap_southeast_2.authority_id, public_ip = module.ap_southeast_2.public_ip }
    "sa-east-1"      = { authority_id = module.sa_east_1.authority_id, public_ip = module.sa_east_1.public_ip }
  }
}

output "artifact_bucket" {
  description = "Name of the S3 bucket holding binary + genesis + configs."
  value       = aws_s3_bucket.artifacts.id
}

output "codebuild_project" {
  description = "CodeBuild project name. Used by scripts/perf/build.sh to start builds."
  value       = aws_codebuild_project.musl.name
}
