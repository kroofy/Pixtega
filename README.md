# Pixtega

An on-demand image derivation service. It fetches an image from a
configured Source (HTTP(S), filesystem, or S3), derives a bounded variant
(width, format, optional quality), and returns cacheable image bytes.

The Rust crate and binary are also named `pixtega`.

This README is the reference for the service's behavior and operation:
URL contract, caching, error taxonomy, configuration, and deployment.

## Documentation

The docs site sources live in [website/](website/) (a static Cloudflare
Workers assets site):

- [Home](website/public/index.html) — what Pixtega is and why
- [Getting started](website/public/docs/getting-started.html) — local and Docker
- [Configuration](website/public/docs/configuration.html) — precedence, schema, limits
- [API reference](website/public/docs/api.html) — URL grammar, caching, errors
- [AWS Lambda](website/public/docs/aws-lambda.html) — recommended serverless path

For deployment on AWS Lambda, see the step-by-step guide in
[deploy/lambda/README.md](deploy/lambda/README.md) and the Lambda container
image in [Dockerfile.lambda](Dockerfile.lambda).

## Quick start (bundled fixtures, no network)

```bash
cargo run -- config.local.toml
# in another shell:
curl -o out.webp 'http://127.0.0.1:8080/images/fixtures/photos/example.jpg/w640.webp'
```

## URL contract

```text
GET /images/{mount}/{source-path}/{transform}[?v={source-version}]
```

Example:

```text
GET /images/public/photos/example.jpg/w1280.webp?v=7d91c2
```

- `{mount}` selects a configured Source. Callers can never supply a
  hostname, bucket, directory, or transport.
- `{source-path}` is everything between the mount and the final segment.
  It is opaque, decoded exactly once, and strictly validated (no traversal,
  no encoded delimiters, canonical percent encoding only).
- `{transform}` is the final segment: `w{width}[,q{quality}].{format}` with
  `format` one of `webp`, `avif`, `jpeg` (no aliases). Width must be in the
  configured allowlist; quality must be in the selected format's allowlist.
  Omitting `q` uses the format's default; spelling out the default is
  rejected so each derived image has exactly one URL.
- `v` is an opaque version token (`[A-Za-z0-9._~-]{1,128}`) used only for
  cache policy. The service never appends it to the upstream key. Callers
  must change `v` whenever the source bytes change.
- Only `GET` is supported (anything else is 405). Request targets over
  8192 bytes are rejected.

The service never upscales: a source narrower than the requested width
keeps its original dimensions.

### Caching

| Response | Cache-Control |
| --- | --- |
| 200 with `v` | `public, max-age=31536000, immutable` |
| 200 without `v` | `public, max-age={unversioned_success_ttl_seconds}` |
| 404 | `public, max-age={not_found_ttl_seconds}` |
| every other error | `no-store` |

Do not enable year-long immutable caching unless every caller changes `v`
when source bytes change; the service cannot verify this for you.

### Error taxonomy

Errors are JSON: `{ "error": "message" }`.

| Status | Meaning |
| --- | --- |
| 400 | invalid path, mount, query, transform, width, format, or quality |
| 404 | the Source answered authoritatively that the object is absent |
| 405 | method other than GET |
| 500 | resize/flatten/encode failed after a valid source was accepted |
| 502 | Source answered without a usable object: permission denied, unexpected upstream status, oversized content, undecodable or unsupported image bytes |
| 504 | fetching the Source timed out |

Permission denial is always 502, never 404, so a misconfigured bucket
cannot be cached as "missing".

## Configuration

TOML, passed as the first CLI argument or via `CONFIG_FILE` (inline TOML
via `CONFIG` is also supported). The schema is closed: unknown fields stop
startup, as does any invalid value. See [config.example.toml](config.example.toml)
for the full annotated shape; every validation rule is enforced at startup
with a specific error message.

Key limits:

| Field | Range | Purpose |
| --- | --- | --- |
| `allowed_widths` | each in 1..=16384 | the closed Width Allowlist |
| `max_download_bytes` | 1..=104857600 | source size cap, enforced on the advertised length and again while streaming |
| `max_source_megapixels` | 1..=500 | decode bomb guard, checked from header metadata before decoding |
| `download_timeout_ms` | 1..=60000 | one timeout over the whole source exchange |
| `max_redirects` | 0..=10 | HTTP(S) redirect bound (same origin, same base path only) |
| `max_concurrent_derivations` | 1..=64 | process-wide fetch+process permits |
| `unversioned_success_ttl_seconds` | 1..=86400 | cache lifetime without `v` |
| `not_found_ttl_seconds` | 1..=3600 | cache lifetime of 404s |

Each enabled format gets its own `[formats.<name>]` block with a required
`default_quality` and an optional `allowed_qualities` list (empty or absent
means callers cannot pass `q` for that format at all).

Sources:

```toml
[[sources]]
mount = "public"          # [a-z][a-z0-9-]{0,31}, unique
key_prefix = "media"      # omitted => defaults to the mount; "" => none
transport = "http"        # or "filesystem" or "s3"
base_url = "https://images.example.test"
```

- `http`: `base_url` (http or https) plus optional `ca_certificate_file`
  (PEM, HTTPS only, resolved relative to the config file) for local fixture
  servers. Hostname verification always stays on. Use HTTPS outside
  loopback and trusted private networks.
- `filesystem`: `root`, resolved relative to the config file and
  canonicalized at startup. Symlinks below the root are always rejected.
- `s3`: `bucket` and `region`, plus optional `endpoint_url` and
  `force_path_style` for local S3-compatible servers.

## S3 credentials and permissions

Credentials are never read from TOML. The adapter uses the standard AWS SDK
credential provider chain: environment variables
(`AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`), shared config/credentials
files and profiles, and instance/task roles.

The service needs `s3:GetObject` on the configured prefix. Note that with
`GetObject` permission alone, S3 answers a missing key with 403 instead of
404 unless the principal also has `s3:ListBucket`; without list permission
every missing object is reported (correctly, per the error taxonomy) as
502 source-unavailable rather than 404. Grant `s3:ListBucket` on the bucket
if you want missing keys to become cacheable 404s.

## Running

### Local (Linux)

```bash
# Debian/Ubuntu build dependencies
sudo apt-get install libvips-dev libheif-plugin-aomenc pkg-config
cargo run -- config.local.toml
```

Ubuntu splits libheif encoders into plugin packages; without
`libheif-plugin-aomenc`, AVIF encoding is unavailable and the service will
refuse to start when `[formats.avif]` is enabled (it verifies every enabled
encoder at startup).

### Local (macOS)

```bash
brew install vips pkg-config
cargo run -- config.local.toml
```

Homebrew's `vips` includes AVIF support via libheif.

### Container

```bash
docker build -t pixtega .
docker run --rm -p 8080:8080 pixtega            # serves config.example.toml
docker run --rm -p 8080:8080 \
  -v "$PWD/myconfig.toml:/config/config.toml:ro" \
  pixtega /config/config.toml
```

### AWS Lambda

[Dockerfile.lambda](Dockerfile.lambda) builds the same image with the AWS
Lambda Web Adapter extension, configured inline via the `CONFIG`
environment variable. The full walkthrough (ECR, IAM, Function URL, curl
smoke test) is in [deploy/lambda/README.md](deploy/lambda/README.md).

## Observability

One structured JSON completion event per request on stdout, with status, a
stable outcome from a closed set (`success`, `rejected_request`,
`not_found`, `timeout`, `source_too_large`, `source_unavailable`,
`undecodable_source`, `resize_failed`, `flatten_failed`, `encode_failed`),
mount, output width/format, upstream status, byte counts, and elapsed
milliseconds. Response bodies, credentials, and `v` values are never
logged.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for build setup and PR expectations.

```bash
./scripts/check.sh   # formatting, clippy, unit + integration tests
```

Tests run entirely against loopback fixture servers (including local
S3-compatible and TLS fixtures) and need no credentials or public network
access.

Mutation testing:

```bash
cargo install cargo-mutants
cargo mutants
```

Configuration and exclusions (with reasons) live in
[.cargo/mutants.toml](.cargo/mutants.toml); the semantic-mutant manifest is
in [docs/semantic-mutants.md](docs/semantic-mutants.md).

## License

MIT. See [LICENSE](LICENSE) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
