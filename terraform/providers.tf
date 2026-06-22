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
      Project     = "suwappu-dag"
      ManagedBy   = "terraform"
      Repository  = "Suwappu-Labs/suwappu-dag"
      Environment = var.environment
    }
  }
}
