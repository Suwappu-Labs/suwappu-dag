# Multi-region provider aliases for the suwappu-devnet.
#
# Forked from terraform/perf/providers.tf. Differences vs. perf:
#   - 4 regions only (us-east-1, eu-west-1, ap-southeast-1, sa-east-1)
#     covering 4 continents without LTP-corridor coupling.
#   - `Component = devnet` default tag so cost-allocation reports
#     split devnet from perf cleanly.

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
    Component  = "devnet"
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
  alias   = "eu_west_1"
  profile = "gsn"
  region  = "eu-west-1"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "ap_southeast_1"
  profile = "gsn"
  region  = "ap-southeast-1"
  default_tags { tags = local.common_tags }
}

provider "aws" {
  alias   = "sa_east_1"
  profile = "gsn"
  region  = "sa-east-1"
  default_tags { tags = local.common_tags }
}
