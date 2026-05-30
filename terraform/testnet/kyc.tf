# KYC vendor (Persona) webhook + DynamoDB record-keeping. Phase B
# per `~/.claude/plans/validated-prancing-curry.md`.
#
# Flow:
#   1. Operator submits the Typeform at apply.testnet.gsx.* (apply.tf).
#   2. Typeform's embedded Persona inquiry widget runs the
#      government-ID + selfie capture inline.
#   3. Persona POSTs an inquiry-completed webhook to the API
#      Gateway HTTP endpoint provisioned here.
#   4. Lambda validates Persona's HMAC signature, extracts
#      (operator_email, inquiry_id, status, candidate ML-DSA pubkey
#      hash), upserts into the `gsx_testnet_applications`
#      DynamoDB table.
#   5. Foundation engineer reviews `#testnet-operator-applications`
#      Slack channel (Persona's standard Slack integration handles
#      that — no extra plumbing here).
#   6. After human approval, runs `scripts/testnet/admit-operator.sh`,
#      which pre-checks DynamoDB and asserts `status = "approved"`
#      before submitting the Intent::AdmitAuthority.
#
# Persona secrets (API key, HMAC webhook secret) live in AWS
# Secrets Manager at gsx-testnet/kyc/persona — set out of band
# when the foundation creates the Persona account. The Lambda
# reads them at boot (via the IAM role attached below).

variable "kyc_webhook_path" {
  description = "URL path the Persona webhook posts to. Persona's webhook config in their dashboard MUST be configured to hit `https://kyc.testnet.gsx.globalsettlement.com/<this-path>`."
  type        = string
  default     = "/persona-webhook"
}

# DynamoDB table holding one row per submitted application.
# PK = candidate's ML-DSA pubkey hash (the same blake3 hash the
# Authority Ring uses for the signer lookup, so admit-operator.sh
# can query without round-tripping through Persona's API).
resource "aws_dynamodb_table" "applications" {
  provider     = aws.us_east_1
  name         = "gsx_testnet_applications"
  billing_mode = "PAY_PER_REQUEST"
  hash_key     = "candidate_pubkey_hash"

  attribute {
    name = "candidate_pubkey_hash"
    type = "S"
  }

  # GSI on inquiry_id so the Lambda can fast-look-up the row when
  # Persona reposts an event (idempotency).
  attribute {
    name = "inquiry_id"
    type = "S"
  }
  global_secondary_index {
    name            = "by_inquiry_id"
    hash_key        = "inquiry_id"
    projection_type = "ALL"
  }

  point_in_time_recovery {
    enabled = true # PII; cheap insurance against accidental delete
  }

  server_side_encryption {
    enabled = true # AWS-managed key; per AWS recs for PII
  }

  tags = { Name = "gsx-testnet-applications" }
}

# Secrets Manager entry for Persona API + webhook HMAC secret.
# Populate via:
#   AWS_PROFILE=gsn aws secretsmanager put-secret-value \
#     --region us-east-1 \
#     --secret-id gsx-testnet/kyc/persona \
#     --secret-string '{"api_key":"...","webhook_secret":"..."}'
# Foundation operator does this after creating the Persona account.
resource "aws_secretsmanager_secret" "persona" {
  provider                = aws.us_east_1
  name                    = "gsx-testnet/kyc/persona"
  description             = "Persona API key + webhook HMAC secret. Populate via secretsmanager put-secret-value after creating the Persona account."
  recovery_window_in_days = 7
  tags                    = { Name = "gsx-testnet-kyc-persona" }
}

# Lambda IAM role.
resource "aws_iam_role" "persona_webhook" {
  provider = aws.us_east_1
  name     = "gsx-testnet-persona-webhook"

  assume_role_policy = jsonencode({
    Version = "2012-10-17"
    Statement = [{
      Effect    = "Allow"
      Principal = { Service = "lambda.amazonaws.com" }
      Action    = "sts:AssumeRole"
    }]
  })
}

resource "aws_iam_role_policy" "persona_webhook" {
  provider = aws.us_east_1
  name     = "persona-webhook"
  role     = aws_iam_role.persona_webhook.id

  policy = jsonencode({
    Version = "2012-10-17"
    Statement = [
      {
        Effect = "Allow"
        Action = [
          "logs:CreateLogGroup",
          "logs:CreateLogStream",
          "logs:PutLogEvents",
        ]
        Resource = ["arn:aws:logs:*:*:*"]
      },
      {
        Effect   = "Allow"
        Action   = ["secretsmanager:GetSecretValue"]
        Resource = [aws_secretsmanager_secret.persona.arn]
      },
      {
        Effect = "Allow"
        Action = [
          "dynamodb:PutItem",
          "dynamodb:UpdateItem",
          "dynamodb:GetItem",
          "dynamodb:Query",
        ]
        Resource = [
          aws_dynamodb_table.applications.arn,
          "${aws_dynamodb_table.applications.arn}/index/*",
        ]
      },
    ]
  })
}

# Lambda function. Source is a thin JS handler in
# `lambda/persona-webhook/index.js`; we package and upload via
# `archive_file`. Runtime is nodejs20 — minimal cold-start, no
# native modules.
data "archive_file" "persona_webhook" {
  type        = "zip"
  source_dir  = "${path.module}/../../lambda/persona-webhook"
  output_path = "${path.module}/persona-webhook.zip"
}

resource "aws_lambda_function" "persona_webhook" {
  provider         = aws.us_east_1
  function_name    = "gsx-testnet-persona-webhook"
  role             = aws_iam_role.persona_webhook.arn
  handler          = "index.handler"
  runtime          = "nodejs20.x"
  filename         = data.archive_file.persona_webhook.output_path
  source_code_hash = data.archive_file.persona_webhook.output_base64sha256
  timeout          = 10
  memory_size      = 256

  environment {
    variables = {
      DDB_TABLE          = aws_dynamodb_table.applications.name
      PERSONA_SECRET_ID  = aws_secretsmanager_secret.persona.arn
    }
  }

  tags = { Name = "gsx-testnet-persona-webhook" }
}

# API Gateway HTTP API in front of the Lambda. HTTP API (v2) is
# cheaper + lower-latency than REST API (v1) and supports JWT/HMAC
# auth at the Lambda layer, which is what Persona's webhook uses
# anyway.
resource "aws_apigatewayv2_api" "kyc" {
  provider      = aws.us_east_1
  name          = "gsx-testnet-kyc"
  protocol_type = "HTTP"
  description   = "Persona webhook ingress for the gsx-testnet operator program"
}

resource "aws_apigatewayv2_integration" "persona_webhook" {
  provider               = aws.us_east_1
  api_id                 = aws_apigatewayv2_api.kyc.id
  integration_type       = "AWS_PROXY"
  integration_uri        = aws_lambda_function.persona_webhook.invoke_arn
  integration_method     = "POST"
  payload_format_version = "2.0"
}

resource "aws_apigatewayv2_route" "persona_webhook" {
  provider  = aws.us_east_1
  api_id    = aws_apigatewayv2_api.kyc.id
  route_key = "POST ${var.kyc_webhook_path}"
  target    = "integrations/${aws_apigatewayv2_integration.persona_webhook.id}"
}

resource "aws_apigatewayv2_stage" "kyc" {
  provider    = aws.us_east_1
  api_id      = aws_apigatewayv2_api.kyc.id
  name        = "$default"
  auto_deploy = true
}

resource "aws_lambda_permission" "kyc_invoke" {
  provider      = aws.us_east_1
  statement_id  = "AllowExecutionFromAPIGateway"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.persona_webhook.function_name
  principal     = "apigateway.amazonaws.com"
  source_arn    = "${aws_apigatewayv2_api.kyc.execution_arn}/*/*"
}

# Custom domain on the wildcard cert. kyc.testnet.gsx.* is the
# public URL Persona's dashboard is configured to post to.
resource "aws_apigatewayv2_domain_name" "kyc" {
  provider    = aws.us_east_1
  domain_name = "kyc.${var.testnet_subdomain}.${var.apex_domain}"

  domain_name_configuration {
    certificate_arn = aws_acm_certificate_validation.wildcard.certificate_arn
    endpoint_type   = "REGIONAL"
    security_policy = "TLS_1_2"
  }
}

resource "aws_apigatewayv2_api_mapping" "kyc" {
  provider    = aws.us_east_1
  api_id      = aws_apigatewayv2_api.kyc.id
  domain_name = aws_apigatewayv2_domain_name.kyc.domain_name
  stage       = aws_apigatewayv2_stage.kyc.id
}

resource "aws_route53_record" "kyc" {
  provider = aws.us_east_1
  zone_id  = aws_route53_zone.testnet.zone_id
  name     = "kyc.${var.testnet_subdomain}.${var.apex_domain}"
  type     = "A"

  alias {
    name                   = aws_apigatewayv2_domain_name.kyc.domain_name_configuration[0].target_domain_name
    zone_id                = aws_apigatewayv2_domain_name.kyc.domain_name_configuration[0].hosted_zone_id
    evaluate_target_health = false
  }
}

output "kyc_webhook_url" {
  description = "Public URL Persona's dashboard MUST be configured to POST to. The HMAC secret in the gsx-testnet/kyc/persona Secrets Manager entry must match the one Persona is configured with."
  value       = "https://${aws_apigatewayv2_domain_name.kyc.domain_name}${var.kyc_webhook_path}"
}

output "kyc_applications_table" {
  description = "DynamoDB table name holding application + KYC status rows. admit-operator.sh queries this before submitting AdmitAuthority."
  value       = aws_dynamodb_table.applications.name
}
