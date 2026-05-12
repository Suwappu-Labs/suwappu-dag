terraform {
  required_version = ">= 1.6.0"

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.60"
    }
  }
}

provider "aws" {
  profile = "gsn"
  region  = var.region

  default_tags {
    tags = {
      Project     = "gsx-dag"
      ManagedBy   = "terraform"
      Repository  = "GlobalSettlementNetwork/gsx-dag"
      Environment = var.environment
    }
  }
}
