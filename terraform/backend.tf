terraform {
  backend "s3" {
    bucket         = "suwappu-dag-tf-state"
    key            = "suwappu-dag/terraform.tfstate"
    region         = "us-east-1"
    profile        = "gsn"
    dynamodb_table = "suwappu-dag-tf-locks"
    encrypt        = true
  }
}
