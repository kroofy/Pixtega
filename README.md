# Pixtega

[![CI](https://github.com/kroofy/Pixtega/actions/workflows/ci.yml/badge.svg)](https://github.com/kroofy/Pixtega/actions/workflows/ci.yml)
[![Mutants](https://github.com/kroofy/Pixtega/actions/workflows/mutants.yml/badge.svg)](https://github.com/kroofy/Pixtega/actions/workflows/mutants.yml)

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
  must change `v` whenever the source bytes change. The `version_token`
  configuration key can downgrade (`ignore`) or reject (`reject`) it — see
  Configuration below.
- `GET` and `HEAD` are supported; `HEAD` returns exactly the `GET`
  response — same status and headers, including `Content-Length` — with
  the body dropped. Anything else is 405 with `Allow: GET, HEAD`. Request
  targets over 8192 bytes are rejected.

The service never upscales: a source narrower than the requested width
keeps its original dimensions.

### Caching

| Response | Cache-Control |
| --- | --- |
| 200 with `v` | `public, max-age=31536000, immutable` |
| 200 without `v` | `public, max-age={unversioned_success_ttl_seconds}` |
| 304 | same policy the matching 200 would have used |
| 404 | `public, max-age={not_found_ttl_seconds}` |
| every other error | `no-store` |

Do not enable year-long immutable caching unless every caller changes `v`
when source bytes change; the service cannot verify this for you. The
table shows the default `version_token = "accept"`: with `"ignore"` a 200
with `v` uses the unversioned policy instead, and with `"reject"` any `v`
is a 400.

A 200 carries an `ETag` when the Source exposed an object identity (S3
`ETag`/`VersionId`, HTTP `ETag`, or filesystem mtime+size+key). The tag
is that identity plus the resolved Transform. Weak upstream tags stay
weak; filesystem identity is always weak. A caller (or CDN) that sends
`If-None-Match` gets `304 Not Modified` when the identity still matches,
without a source-body fetch or encode. Identify runs before a derivation
permit, so a matching revalidation does not queue behind encodes. A HEAD
the origin refuses (other than timeout) is ignored and the service
fetches instead. No identity, or a mismatch, is a normal 200. There is
no `Last-Modified`.

### Error taxonomy

Errors are JSON: `{ "error": "message" }`.

| Status | Meaning |
| --- | --- |
| 400 | invalid path, mount, query, transform, width, format, or quality |
| 404 | the Source answered authoritatively that the object is absent |
| 405 | method other than GET or HEAD |
| 500 | flatten/encode failed after a valid source was accepted |
| 502 | Source answered without a usable object: permission denied, unexpected upstream status, oversized content, undecodable or unsupported image bytes |
| 504 | fetching the Source timed out, or the whole request exceeded `request_timeout_ms` |

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
| `download_timeout_ms` | 1..=60000 | one timeout over the whole source exchange; never larger than `request_timeout_ms` |
| `request_timeout_ms` | 1..=300000 | whole-request deadline (optional, default 9000); permit wait, fetch, and processing spend from this one budget |
| `max_redirects` | 0..=10 | HTTP(S) redirect bound (same origin, same base path only) |
| `max_concurrent_derivations` | 1..=64 | process-wide fetch+process permits |
| `unversioned_success_ttl_seconds` | 1..=86400 | cache lifetime without `v` |
| `not_found_ttl_seconds` | 1..=3600 | cache lifetime of 404s |

`request_timeout_ms` is the deadline for answering one request: waiting for
a `max_concurrent_derivations` permit, the source fetch (bounded by
`download_timeout_ms`, which must not exceed it), and decode/resize/encode
all spend from the same budget, and expiry is a real 504 with
`Cache-Control: no-store` — a request queued behind slow derivations can
therefore time out before its fetch or processing even starts. Keep the
deadline below any host kill deadline
(for example an AWS Lambda function timeout), so the service answers before
the host tears the response down mid-stream. A timed-out encode cannot be
cancelled: it keeps occupying its `max_concurrent_derivations` slot until
libvips returns.

`version_token` (optional, default `"accept"`) decides what the `v` query
parameter means; the value set is closed and anything else stops startup:

- `accept`: a valid `v` upgrades the response to year-long immutable
  caching (the default and pre-existing behavior).
- `ignore`: `v` is parsed and validated exactly as in `accept`, but the
  response is served with `unversioned_success_ttl_seconds` as if no `v`
  were present. A well-formed `v` is never a 400.
- `reject`: any `v` is a 400, like any other unknown query parameter, for
  deployments that version sources by path. A missing `v` is always fine.

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
  loopback and trusted private networks; plain `http` is supported
  intentionally for local fixtures and trusted internal hops only.
- `filesystem`: `root`, resolved relative to the config file and
  canonicalized at startup. Symlinks below the root are always rejected.
- `s3`: `bucket` and `region`, plus optional `endpoint_url` and
  `force_path_style` for local S3-compatible servers. Transport failures
  that never got an HTTP response (stale connection, DNS, TLS reset) are
  retried inside `download_timeout_ms`; service errors (403, 404, 5xx)
  stay one attempt.

### Pinning sources to trusted origins

Every upstream fetch destination is fixed by the operator: callers select
only a mount, never a host. Pin each `base_url` (and S3 `endpoint_url`) to
an origin you control, over HTTPS wherever it crosses a network you do not.

On top of that, a destination policy is enforced at startup and again on
every fetch: a `base_url` or `endpoint_url` whose host is loopback,
link-local (where cloud instance-metadata endpoints live), unspecified,
RFC 1918 private, carrier-grade NAT, IPv6 unique-local, `localhost`, or a
reserved-internal name (`metadata`, `*.internal`) is a startup error. The
HTTP adapter re-checks the same policy before every connection, including
each redirect hop — redirects are additionally bounded and must stay on the
configured scheme, host, port, and base path.

For local development against fixture servers, opt out per source:

```toml
[[sources]]
mount = "dev"
transport = "http"
base_url = "http://127.0.0.1:9000"
allow_private_destinations = true   # local development only
```

The policy checks the literal configured host and does not resolve DNS, so
it cannot detect a public hostname that resolves to a private address —
another reason to point sources only at origins you control.

## S3 credentials and permissions

Credentials are never read from TOML. The adapter uses the standard AWS SDK
credential provider chain: environment variables
(`AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`), shared config/credentials
files and profiles, and instance/task roles.

The service needs `s3:GetObject` on the configured prefix. With
`GetObject` alone, S3 answers a missing key with 403, which the service
reports (correctly, per the error taxonomy) as 502 source-unavailable.
Grant `s3:ListBucket` on the bucket if you want missing keys to become
cacheable 404s.

## Running

### Local (Linux)

```bash
# Debian/Ubuntu build dependencies (libvips 8.17+ required; Ubuntu 26.04+)
sudo apt-get install libvips-dev libheif-plugin-aomenc pkg-config
cargo run -- config.local.toml
```

The pinned libvips bindings (`=2.3.0`) pass argument names introduced in
libvips 8.17, so older distribution packages (e.g. Ubuntu 24.04's libvips
8.15) fail at runtime. Ubuntu splits libheif encoders into plugin packages; without
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

## JavaScript client

Pure URL builders (`pixtegaUrl`, `pixtegaSrcSet`, `pixtegaPicture`) for
this service's URL contract live in [js/](js/), published to npm from
each release tag as [`pixtega`](https://www.npmjs.com/package/pixtega)
and [`@pixtega/url`](https://www.npmjs.com/package/@pixtega/url), a thin
re-export with the same API:

```bash
npm install pixtega     # or: npm install @pixtega/url
```

## Observability

One structured JSON completion event per request on stdout, with status, a
stable outcome from a closed set (`success`, `rejected_request`,
`not_found`, `timeout`, `source_too_large`, `source_unavailable`,
`undecodable_source`, `flatten_failed`, `encode_failed`),
mount, output width/format, upstream status, byte counts, and elapsed
milliseconds. When a source failure carries an internal `detail` (for
example `s3 dispatch failure` vs `s3 service error: AccessDenied`), a
separate `{"event":"source_error","level":"warn","detail":"..."}` line is
emitted immediately before `request_completed`. `detail` is for logs only
and never appears in the client JSON body. Response bodies, credentials,
and `v` values are never logged.

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

## CI, mutation score, and releases

Full details in [docs/ci.md](docs/ci.md).

- **CI** runs `./scripts/check.sh` and
  `./scripts/container-acceptance.sh` on every PR and push to `main`.
- **Mutation score**: the project threshold is ≥ 90%. The Mutants workflow
  (weekly on `main` + manual dispatch) runs the full `cargo mutants`
  suite, publishes the score in the run's job summary and a
  `mutants-report` artifact (`mutants-score.json` + the full
  `mutants.out/` report), and fails below 90%. The latest recorded score
  is in the job summary of the most recent
  [Mutants run](https://github.com/kroofy/Pixtega/actions/workflows/mutants.yml)
  (badge above).
- **Releases**: `git tag vX.Y.Z && git push origin vX.Y.Z` builds and
  pushes `ghcr.io/kroofy/pixtega` images (`X.Y.Z`, `X.Y`, `latest`, and
  the `-lambda` variant from [Dockerfile.lambda](Dockerfile.lambda)) and
  creates a GitHub Release with the binary tarball and SHA256 checksums
  attached.

## License

MIT. See [LICENSE](LICENSE) and
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
