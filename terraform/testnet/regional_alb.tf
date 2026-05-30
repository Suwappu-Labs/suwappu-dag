# Per-region RPC ALBs — one per seed validator, each in that validator's
# own VPC (so it targets the instance in-region; no cross-VPC/public-IP
# limit). Global Accelerator (ga.tf) fronts all seven behind one anycast
# endpoint. us-east-1 reuses the existing wildcard cert (acm.tf); the
# other six use the regional certs in rpc_certs.tf.

module "rpc_alb_us_east_1" {
  source             = "./modules/regional-rpc-alb"
  providers          = { aws = aws.us_east_1 }
  name_prefix        = "gsx-testnet-"
  region_label       = "us-east-1"
  vpc_id             = module.us_east_1.vpc_id
  subnet_ids         = module.us_east_1.alb_subnet_ids
  target_instance_id = module.us_east_1.instance_id
  target_port        = var.rpc_port
  certificate_arn    = aws_acm_certificate_validation.wildcard.certificate_arn
}

module "rpc_alb_us_west_2" {
  source             = "./modules/regional-rpc-alb"
  providers          = { aws = aws.us_west_2 }
  name_prefix        = "gsx-testnet-"
  region_label       = "us-west-2"
  vpc_id             = module.us_west_2.vpc_id
  subnet_ids         = module.us_west_2.alb_subnet_ids
  target_instance_id = module.us_west_2.instance_id
  target_port        = var.rpc_port
  certificate_arn    = aws_acm_certificate_validation.rpc_us_west_2.certificate_arn
}

module "rpc_alb_eu_west_1" {
  source             = "./modules/regional-rpc-alb"
  providers          = { aws = aws.eu_west_1 }
  name_prefix        = "gsx-testnet-"
  region_label       = "eu-west-1"
  vpc_id             = module.eu_west_1.vpc_id
  subnet_ids         = module.eu_west_1.alb_subnet_ids
  target_instance_id = module.eu_west_1.instance_id
  target_port        = var.rpc_port
  certificate_arn    = aws_acm_certificate_validation.rpc_eu_west_1.certificate_arn
}

module "rpc_alb_eu_central_1" {
  source             = "./modules/regional-rpc-alb"
  providers          = { aws = aws.eu_central_1 }
  name_prefix        = "gsx-testnet-"
  region_label       = "eu-central-1"
  vpc_id             = module.eu_central_1.vpc_id
  subnet_ids         = module.eu_central_1.alb_subnet_ids
  target_instance_id = module.eu_central_1.instance_id
  target_port        = var.rpc_port
  certificate_arn    = aws_acm_certificate_validation.rpc_eu_central_1.certificate_arn
}

module "rpc_alb_ap_southeast_1" {
  source             = "./modules/regional-rpc-alb"
  providers          = { aws = aws.ap_southeast_1 }
  name_prefix        = "gsx-testnet-"
  region_label       = "ap-southeast-1"
  vpc_id             = module.ap_southeast_1.vpc_id
  subnet_ids         = module.ap_southeast_1.alb_subnet_ids
  target_instance_id = module.ap_southeast_1.instance_id
  target_port        = var.rpc_port
  certificate_arn    = aws_acm_certificate_validation.rpc_ap_southeast_1.certificate_arn
}

module "rpc_alb_ap_northeast_1" {
  source             = "./modules/regional-rpc-alb"
  providers          = { aws = aws.ap_northeast_1 }
  name_prefix        = "gsx-testnet-"
  region_label       = "ap-northeast-1"
  vpc_id             = module.ap_northeast_1.vpc_id
  subnet_ids         = module.ap_northeast_1.alb_subnet_ids
  target_instance_id = module.ap_northeast_1.instance_id
  target_port        = var.rpc_port
  certificate_arn    = aws_acm_certificate_validation.rpc_ap_northeast_1.certificate_arn
}

module "rpc_alb_sa_east_1" {
  source             = "./modules/regional-rpc-alb"
  providers          = { aws = aws.sa_east_1 }
  name_prefix        = "gsx-testnet-"
  region_label       = "sa-east-1"
  vpc_id             = module.sa_east_1.vpc_id
  subnet_ids         = module.sa_east_1.alb_subnet_ids
  target_instance_id = module.sa_east_1.instance_id
  target_port        = var.rpc_port
  certificate_arn    = aws_acm_certificate_validation.rpc_sa_east_1.certificate_arn
}
