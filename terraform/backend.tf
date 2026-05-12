terraform {
  backend "s3" {
    bucket         = "gsx-dag-tf-state"
    key            = "gsx-dag/terraform.tfstate"
    region         = "us-east-1"
    profile        = "gsn"
    dynamodb_table = "gsx-dag-tf-locks"
    encrypt        = true
  }
}
