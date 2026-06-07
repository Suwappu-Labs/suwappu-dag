# Remote state. Separate key from devnet + perf so the three
# environments apply independently and never share locks.

terraform {
  backend "s3" {
    bucket         = "suwappu-dag-tf-state"
    key            = "suwappu-dag/testnet/terraform.tfstate"
    region         = "us-east-1"
    profile        = "gsn"
    dynamodb_table = "suwappu-dag-tf-locks"
    encrypt        = true
  }
}
