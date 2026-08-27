# Deploying Pixtega on AWS Lambda

This guide deploys the service as a Lambda container image behind a
[Function URL](https://docs.aws.amazon.com/lambda/latest/dg/lambda-urls.html).
The image is built from [`Dockerfile.lambda`](../../Dockerfile.lambda), which
is the standard image plus the
[AWS Lambda Web Adapter](https://github.com/aws/aws-lambda-web-adapter)
extension. The adapter translates Lambda invocations into plain HTTP against
the service, so no Lambda-specific code exists in the binary — the same image
also runs on ECS, EC2, or your laptop.

Configuration is supplied inline through the `CONFIG` environment variable
(the full TOML document), so changing configuration never requires an image
rebuild. See [`example-config.toml`](example-config.toml) for a starting
point that serves the bundled fixtures and shows an S3 source.

## Prerequisites

- AWS CLI v2, authenticated with permissions for ECR, Lambda, and IAM
- Docker (if building locally, build for the architecture you will run:
  `linux/amd64` below; use `--platform linux/arm64` and
  `--architectures arm64` consistently if you prefer Graviton)

## 1. Build the image (or pull a released one)

Released images on GHCR are multi-arch (`linux/amd64` and `linux/arm64`),
so you can skip the build and mirror a tag into ECR instead — Graviton
included:

```bash
docker pull ghcr.io/kroofy/pixtega:lambda   # or a pinned X.Y.Z-lambda tag
docker tag ghcr.io/kroofy/pixtega:lambda pixtega-lambda
```

To build locally, from the repository root:

```bash
docker build -f Dockerfile.lambda --platform linux/amd64 -t pixtega-lambda .
```

## 2. Push to ECR

```bash
AWS_ACCOUNT_ID=$(aws sts get-caller-identity --query Account --output text)
AWS_REGION=us-east-1
ECR_REPO="${AWS_ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com/pixtega"

aws ecr create-repository --repository-name pixtega --region "$AWS_REGION"
aws ecr get-login-password --region "$AWS_REGION" \
  | docker login --username AWS --password-stdin \
      "${AWS_ACCOUNT_ID}.dkr.ecr.${AWS_REGION}.amazonaws.com"

docker tag pixtega-lambda "${ECR_REPO}:latest"
docker push "${ECR_REPO}:latest"
```

## 3. Create the execution role

```bash
aws iam create-role --role-name pixtega-lambda \
  --assume-role-policy-document '{
    "Version": "2012-10-17",
    "Statement": [{
      "Effect": "Allow",
      "Principal": {"Service": "lambda.amazonaws.com"},
      "Action": "sts:AssumeRole"
    }]
  }'
aws iam attach-role-policy --role-name pixtega-lambda \
  --policy-arn arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole
```

### IAM for S3 sources

If your configuration has `transport = "s3"` sources, the execution role is
the credential (the service uses the standard AWS SDK provider chain;
credentials are never read from TOML). Grant it `s3:GetObject` on the
configured prefix:

```bash
aws iam put-role-policy --role-name pixtega-lambda \
  --policy-name pixtega-s3-read \
  --policy-document '{
    "Version": "2012-10-17",
    "Statement": [
      {
        "Effect": "Allow",
        "Action": "s3:GetObject",
        "Resource": "arn:aws:s3:::example-image-bucket/originals/*"
      },
      {
        "Effect": "Allow",
        "Action": "s3:ListBucket",
        "Resource": "arn:aws:s3:::example-image-bucket"
      }
    ]
  }'
```

The `s3:ListBucket` statement is optional but recommended: with `GetObject`
alone, S3 answers a missing key with 403 instead of 404, so every missing
object is reported (correctly, per the error taxonomy) as 502
source-unavailable rather than a cacheable 404.

## 4. Create the function

The `CONFIG` value is the entire TOML configuration document. Keep the
listen address on port 8080 — the Lambda Web Adapter forwards to
`127.0.0.1:8080` by default.

```bash
CONFIG_TOML=$(cat deploy/lambda/example-config.toml)

aws lambda create-function \
  --function-name pixtega \
  --package-type Image \
  --code ImageUri="${ECR_REPO}:latest" \
  --role "arn:aws:iam::${AWS_ACCOUNT_ID}:role/pixtega-lambda" \
  --architectures x86_64 \
  --memory-size 1024 \
  --timeout 30 \
  --environment "Variables={CONFIG=${CONFIG_TOML}}" \
  --region "$AWS_REGION"
```

Notes:

- `Dockerfile.lambda` already sets `AWS_LWA_READINESS_CHECK_PROTOCOL=tcp`
  (the service has no HTTP health endpoint — `GET /` is a 400 by design —
  so the adapter waits for the TCP socket instead of an HTTP 200) and
  `AWS_LWA_INVOKE_MODE=response_stream` (to match the streaming Function
  URL created below) in the image. If you override the image environment,
  keep both variables. Exception: ALB does not support Lambda response
  streaming — behind an ALB, set `AWS_LWA_INVOKE_MODE=buffered` instead.
  API Gateway REST APIs stream only when the Lambda proxy integration sets
  `responseTransferMode=STREAM` (with the `/response-streaming-invocations`
  URI); with a default (buffered) integration, override to
  `AWS_LWA_INVOKE_MODE=buffered` too. Same pairing rule: the adapter's
  mode must match how the function is invoked.
- Lambda CPU scales with memory. Image decode/resize/encode is CPU-bound;
  1024 MB is a reasonable floor, and larger widths or AVIF output benefit
  from more.
- The function environment has a 4 KB total size limit. If your TOML does
  not fit, bake a config file into the image and set `CONFIG_FILE` to its
  path instead.
- Configuration changes are just
  `aws lambda update-function-configuration --function-name pixtega --environment ...`.

## 5. Create a Function URL (smoke test only)

> **Warning:** `--auth-type NONE` makes the URL publicly invokable by
> anyone on the internet, with no CDN cache absorbing traffic and no
> auth layer. Use it only for the smoke test below (and delete it or
> front it afterwards); it is unsafe as a production internet-facing
> endpoint.

```bash
aws lambda create-function-url-config \
  --function-name pixtega \
  --auth-type NONE \
  --invoke-mode RESPONSE_STREAM \
  --region "$AWS_REGION"

aws lambda add-permission \
  --function-name pixtega \
  --action lambda:InvokeFunctionUrl \
  --principal '*' \
  --function-url-auth-type NONE \
  --statement-id public-url \
  --region "$AWS_REGION"
```

### Production exposure

Do not serve production traffic straight off a `NONE`-auth Function URL.
Put CloudFront in front of it:

- Create a CloudFront distribution with the Function URL domain as the
  origin. The service's `Cache-Control` policy (year-long immutable for
  versioned successes, bounded TTLs for the rest) is designed for exactly
  this; most traffic never reaches the function.
- Lock the origin to CloudFront: switch the Function URL to
  `--auth-type AWS_IAM` and attach a CloudFront Origin Access Control
  (OAC) for Lambda Function URLs, so only the distribution can invoke the
  function. Then remove the public `NONE` permission added above
  (`aws lambda remove-permission --function-name pixtega --statement-id public-url`).
- Without OAC, at minimum keep the raw Function URL secret and monitor
  invocations — but IAM auth + OAC is the supported way to prevent
  callers from bypassing the CDN (and its cache) entirely.

## 6. Smoke test

```bash
FUNCTION_URL=$(aws lambda get-function-url-config \
  --function-name pixtega --query FunctionUrl --output text --region "$AWS_REGION")

curl -o out.webp "${FUNCTION_URL}images/fixtures/photos/example.jpg/w640.webp"
file out.webp   # RIFF ... Web/P image
```

The example config's `fixtures` mount serves images bundled in the image at
`/app/fixtures`, so this works before any S3 setup. Then request through an
S3 mount, e.g. `${FUNCTION_URL}images/photos/cat.jpg/w1280.webp?v=1`.

## Caveats

- **Function URL payload limit.** Buffered Function URL responses are
  capped at about 6 MB, and large widths at high JPEG qualities can exceed
  it — so this image and this guide default to response streaming, which
  raises the limit substantially: the image sets
  `AWS_LWA_INVOKE_MODE=response_stream` and the Function URL above is
  created with `--invoke-mode RESPONSE_STREAM`. The two must stay in sync;
  if they disagree, responses come back as the adapter's buffered JSON
  envelope instead of image bytes. To opt out and run buffered, change
  both: create the Function URL as BUFFERED (omit `--invoke-mode` or set
  it explicitly) *and* set `AWS_LWA_INVOKE_MODE=buffered` in the function
  environment — then consider constraining `allowed_widths` and
  `allowed_qualities` so outputs cannot exceed the 6 MB cap. The buffered
  opt-out is mandatory behind an ALB (no streaming support) and behind an
  API Gateway integration that is not `responseTransferMode=STREAM`.
- **Cold starts.** Each cold start loads libvips and validates
  configuration, including verifying every enabled encoder; the first
  request on a new execution environment is noticeably slower than warm
  requests. Use provisioned concurrency if tail latency matters, and put a
  CDN in front so most traffic never reaches the function at all.
- **`max_concurrent_derivations` is per instance.** Lambda sends one
  request at a time to each execution environment (unless you have opted
  into multi-concurrency features), so this limit rarely binds there; it
  still protects you when running the same image on ECS/EC2.
