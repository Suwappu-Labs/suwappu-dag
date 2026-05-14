terraform {
  required_version = ">= 1.5.0"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 5.60"
    }
  }
}

provider "aws" {
  profile = "gsn"
  region  = "us-east-1"

  default_tags {
    tags = {
      Project    = "gsx-dag"
      Component  = "tf-state-bootstrap"
      ManagedBy  = "terraform"
      Repository = "GlobalSettlementNetwork/gsx-dag"
    }
  }
}
