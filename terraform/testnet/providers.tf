# Multi-region provider aliases for the gsx-testnet.
#
# Diff vs. terraform/devnet/:
#   - 7 seed regions instead of 4. The set matches the paper's
#     7-of-9 LTP corridor (paper §10.2) — geographic spread that
#     also gives external operators predictable peering targets.
#   - `Component = testnet` default tag for cost-allocation
#     reports separate from devnet.
#
# External validators (Track B program) do NOT run via this
# provider tree — they bring their own hardware. This stack only
# provisions the foundation seed cluster.

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
    Project    = "gsx-dag"
    Component  = "testnet"
    ManagedBy  = "terraform"
    Repository = "GlobalSettlementNetwork/gsx-dag"
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
  alias   = "eu_central_1"
  profile = "gsn"
  region  = "eu-central-1"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "ap_southeast_1"
  profile = "gsn"
  region  = "ap-southeast-1"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "ap_northeast_1"
  profile = "gsn"
  region  = "ap-northeast-1"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "sa_east_1"
  profile = "gsn"
  region  = "sa-east-1"
  default_tags { tags = local.common_tags }
}
