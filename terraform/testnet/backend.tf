# Remote state. Separate key from devnet + perf so the three
# environments apply independently and never share locks.

terraform {
  backend "s3" {
    bucket         = "gsx-dag-tf-state"
    key            = "gsx-dag/testnet/terraform.tfstate"
    region         = "us-east-1"
    profile        = "gsn"
    dynamodb_table = "gsx-dag-tf-locks"
    encrypt        = true
  }
}
