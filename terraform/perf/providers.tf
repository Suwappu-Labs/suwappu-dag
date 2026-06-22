# Multi-region provider aliases for the perf testnet.
#
# Each region module is instantiated with one of these aliases so a single
# `terraform apply` provisions the full 7-region mesh. Provider config matches
# the root `terraform/providers.tf` (profile = gsn, default tags).

terraform {
  required_version = ">= 1.5.0"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.60"
    }
  }
}

locals {
  common_tags = {
    Project    = "suwappu-dag"
    Component  = "perf-testnet"
    ManagedBy  = "terraform"
    Repository = "Suwappu-Labs/suwappu-dag"
  }
}

provider "aws" {
  alias   = "us_east_1"
  profile = "gsn"
  region  = "us-east-1"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "us_west_2"
  profile = "gsn"
  region  = "us-west-2"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "eu_west_1"
  profile = "gsn"
  region  = "eu-west-1"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "ap_northeast_1"
  profile = "gsn"
  region  = "ap-northeast-1"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "ap_southeast_2"
  profile = "gsn"
  region  = "ap-southeast-2"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "sa_east_1"
  profile = "gsn"
  region  = "sa-east-1"
  default_tags { tags = local.common_tags }
}

# af-south-1 provider intentionally omitted — region requires AWS account
# opt-in and the gsn account is not currently enrolled. See main.tf for the
# commented-out module block. Restoring this provider is the first step of
# the re-enable procedure.
