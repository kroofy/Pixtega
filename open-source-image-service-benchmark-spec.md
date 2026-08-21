# On-demand image derivation service

## Project and benchmark specification

### Purpose

Build a standalone HTTP service that fetches an image from a configured source, derives a bounded
variant, and returns cacheable image bytes.

The service is intended for public use and for coding-agent evaluation. It must run and be tested
without cloud credentials, private infrastructure, or access to a third-party production system.

The hard part is not resizing an image. The hard part is keeping the request space bounded, giving
each derived image one canonical address, distinguishing missing source objects from system
failures, and enforcing resource limits before untrusted input can exhaust the process.

### Required implementation

- Rust, using the stable toolchain pinned by the repository.
- An ordinary HTTP server that can run with `cargo run`.
- libvips for image decoding, resizing, and encoding.
- TOML configuration.
- An OCI-compatible container image.
- Linux and macOS development instructions.
- HTTP(S), filesystem, and S3 Source adapters.
- No dependency on a specific CDN, function runtime, or cloud account.

Cloud-specific deployment modules may be added later, but they are outside the benchmark.

## Domain language

Use these terms in code and documentation.

- **Image Service**: the HTTP application described here.
- **Source**: a configured location from which the service may read.
- **Transport**: the mechanism used to read a Source: HTTP(S), filesystem, or S3.
- **Mount**: the first request-path segment that selects a Source.
- **Key Prefix**: an optional configured prefix added before the requested source path.
- **Source Object**: the bytes read from a Source at one path.
- **Transform**: the requested output width, format, and optional quality.
- **Derived Image**: one Source Object encoded under one Transform.
- **Width Allowlist**: every width the service is willing to derive.

A Source always exists because configuration defines it. A Source Object may be absent.

## Core invariants

1. A request can read only from a configured Source. A caller cannot submit an arbitrary source
   URL or filesystem path.
2. Widths, formats, and optional qualities come from closed allowlists.
3. The source path and Transform each have one canonical spelling. The optional version token
   intentionally creates a new cache identity when source bytes change.
4. The source path remains opaque. Only the Mount and final Transform segment have syntax.
5. The service never upscales an image.
6. A missing Source Object is a normal 404. It is not reported as a service fault.
7. A Source failure and an image-processing failure are different error classes.
8. A versioned Derived Image is immutable for the version named by its URL. An unversioned one has
   a shorter cache lifetime.
9. Request validation happens before a Source is contacted.
10. Configuration errors stop startup rather than producing a server that rejects every request.

## HTTP contract

### Request

The service must support:

```text
GET /images/{mount}/{source-path}/{transform}[?v={source-version}]
```

Example:

```text
GET /images/public/photos/example.jpg/w1280.webp?v=7d91c2
```

The default `/images` prefix is configurable.

The optional `v` query parameter is an opaque Source Object version used for cache keys, cache
busting, and version control. When present, it must match `[A-Za-z0-9._~-]{1,128}` exactly. Percent
encoding is not accepted in `v`. The service does not append it to the upstream key or otherwise
interpret its value.

At most one `v` parameter is accepted. Unknown or repeated query parameters are rejected with 400.
The service must produce the same image bytes with or without `v`; only the response cache policy
changes.

The caller owns the version invariant: it must change `v` whenever the bytes at the Source Object's
key change. The Image Service cannot verify this because `v` does not select an upstream object
version. A deployment must not enable year-long immutable caching unless every caller follows this
rule.

Methods other than `GET` return 405. Supporting `HEAD` is an optional extension.

Reject a request target longer than 8192 bytes with 400.

### Transform grammar

The final path segment has this grammar:

```text
transform := width [ "," quality ] "." format
width     := "w" canonical-decimal
quality   := "q" canonical-decimal
format    := "webp" | "avif" | "jpeg"
```

Examples:

```text
w640.webp
w1280,q65.avif
w1920.jpeg
```

Rules:

- Width is required and appears first.
- Quality is optional and appears second.
- Format is required and lowercase.
- Each field appears at most once.
- A canonical decimal contains ASCII digits only.
- A canonical decimal has no sign and no leading zero unless its entire value is `0`.
- Format aliases such as `jpg` are rejected.
- Extra fields, reordered fields, repeated fields, and extra dots are rejected.
- Width must be in the configured Width Allowlist.
- Format must have a configured format policy.
- Quality is rejected unless it is in the selected format policy's quality allowlist.
- A quality equal to the selected format's default is rejected. Omitting `q` is the canonical
  spelling of the default.

The service must reject a malformed or disallowed Transform before fetching the Source Object.

### Source routing

The Mount selects a configured Source. The caller cannot supply a hostname, bucket, base directory,
or transport.

Everything between the Mount and the final Transform segment is the requested source path. The
service applies the selected Source's Key Prefix to produce the upstream key.

If `key_prefix` is omitted, it defaults to the Mount. If it is the empty string, the requested path
is used unchanged.

Given:

```toml
[[sources]]
mount = "public"
key_prefix = "media"
transport = "http"
base_url = "https://images.example.test"
```

this request:

```text
/images/public/photos/example.jpg/w640.webp?v=1
```

reads:

```text
https://images.example.test/media/photos/example.jpg
```

Mount names must be unique. Duplicate Mounts are a startup error.
They must match `[a-z][a-z0-9-]{0,31}`.

### Path validation

Validate the caller-controlled source path before applying the configured Key Prefix.

Reject:

- an empty source path
- empty path segments
- `.` or `..` segments, including percent-encoded forms
- percent-encoded `/` or `\`
- literal backslashes
- absolute paths
- null bytes and ASCII control characters
- invalid percent encoding
- double-encoded traversal or path-delimiter sequences

On the wire, a source-path segment may contain RFC 3986 unreserved ASCII characters or
percent-encoded UTF-8 bytes. Percent triplets must use uppercase hexadecimal. Reject percent
encoding of an unreserved ASCII character because it would create a second spelling of the same
path. Decode each path segment exactly once and require valid UTF-8.

Do not normalize an invalid path into a valid one. The validator must also reject a segment whose
second decoding would turn the complete segment into `.` or `..`, or would introduce `/` or `\`.
The validated path used to fetch the Source Object must preserve ordinary dots, underscores,
Unicode, and nested segments.

## Configuration

The application loads TOML from a path passed on the command line or through `CONFIG_FILE`.
Supporting inline TOML through `CONFIG` is optional.

Example:

```toml
listen_address = "0.0.0.0:8080"
path_prefix = "/images"

allowed_widths = [320, 640, 1280, 1920]

max_download_bytes = 52428800
max_source_megapixels = 100
download_timeout_ms = 10000
max_redirects = 3
max_concurrent_derivations = 8
unversioned_success_ttl_seconds = 3600
not_found_ttl_seconds = 60

[formats.webp]
default_quality = 82
allowed_qualities = [60, 72, 90]

[formats.avif]
default_quality = 55
allowed_qualities = [40, 65]

[formats.jpeg]
default_quality = 85
allowed_qualities = [70, 92]

[[sources]]
mount = "public"
key_prefix = "media"
transport = "http"
base_url = "https://images.example.test"

[[sources]]
mount = "fixtures"
key_prefix = ""
transport = "filesystem"
root = "./fixtures"

[[sources]]
mount = "archive"
key_prefix = "originals"
transport = "s3"
bucket = "example-image-bucket"
region = "us-east-1"
```

Configuration validation must reject:

- no Sources
- duplicate or invalid Mount names
- a missing or empty Width Allowlist
- width `0`
- duplicate widths
- no format policies
- an unknown or duplicate format policy
- a missing default quality
- a default or allowed quality outside `1..=100`
- duplicate allowed qualities within one format policy
- an allowed quality equal to that format's default
- zero or invalid limits, timeouts, and cache TTLs
- a base URL whose scheme is not HTTP or HTTPS
- a filesystem root that does not resolve to a readable directory
- a Key Prefix containing traversal segments

Resolve a relative filesystem root against the directory containing the configuration file, then
canonicalize it once at startup. Relative paths do not depend on the process working directory.

The schema is closed. Unknown fields are startup errors. Apply these rules:

- `path_prefix` starts with `/`, contains canonical non-empty path segments, and has no trailing
  slash.
- Widths are in `1..=16384`.
- `max_download_bytes` is in `1..=104857600`.
- `max_source_megapixels` is in `1..=500`.
- `download_timeout_ms` is in `1..=60000`.
- `max_redirects` is in `0..=10`.
- `max_concurrent_derivations` is in `1..=64`.
- `unversioned_success_ttl_seconds` is in `1..=86400`.
- `not_found_ttl_seconds` is in `1..=3600`.
- `transport` is exactly `http`, `filesystem`, or `s3`.
- The `http` discriminator covers both HTTP and HTTPS according to the `base_url` scheme.
- An HTTP(S) Source has `base_url` and may have `ca_certificate_file`; it cannot have filesystem or
  S3 fields.
- A filesystem Source has `root`; it cannot have HTTP(S) or S3 fields.
- An S3 Source has `bucket` and `region`, may have `endpoint_url` and `force_path_style`, and cannot
  have HTTP(S) or filesystem fields.
- S3 bucket and region values are non-empty and contain no slash or control character.
- An S3 `endpoint_url`, when present, uses HTTP or HTTPS.
- Credentials are never accepted in TOML.
- `key_prefix` is a relative path with no empty, `.` or `..` segment.

Each format policy owns its encoder default and quality allowlist because quality scales are not
comparable across codecs. A missing `allowed_qualities` in one policy defaults to empty for that
format only. This means callers cannot specify `q` for that format; it does not mean all qualities
are accepted.

The default quality has no implicit fallback and must be configured for every enabled format. The
request parser and encoder must read the same resolved format policy so canonicalization and
encoding cannot disagree.

## Source adapters

The Image Service module must depend on a small internal Source interface. HTTP(S), filesystem, and
S3 are adapters behind that seam. Request parsing and image processing must not contain
transport-specific branches.

### HTTP(S) adapter

- Join the configured base URL, Key Prefix, and validated source path without changing path
  semantics. Append path segments with a URL parser, not string concatenation.
- Accept HTTP and HTTPS base URLs. Source configuration is trusted operator input, never caller
  input. Documentation must recommend HTTPS outside loopback and trusted private networks.
- Support an optional PEM CA certificate file so local fixture servers can use a test certificate.
  The field is named `ca_certificate_file` and is valid only for an HTTPS base URL. Resolve its path
  relative to the configuration file. TLS hostname verification remains enabled.
- Apply one timeout to the whole exchange, including body streaming.
- Follow no more than the configured number of redirects.
- Redirects must keep the original scheme, host, and effective port, and remain beneath the
  configured base path.
- Send `Accept-Encoding: identity` and reject a response with any other content encoding. This
  keeps byte-limit accounting unambiguous.
- Treat 404 and 410 as Source Object absence.
- Treat every other non-2xx response as Source unavailability.
- Check `Content-Length` when present.
- Enforce the byte limit again while streaming because the header may be absent or false.
- Never include credentials or arbitrary response bodies in a client-facing error.

### Filesystem adapter

- Resolve paths beneath the configured absolute root.
- Reject every symlink encountered below the root, whether it points inside or outside. The
  benchmark tests static paths and does not test concurrent filesystem replacement.
- Treat a missing regular file as Source Object absence.
- Reject directories and non-regular files as Source unavailability.
- Enforce the same byte limit as the HTTP(S) adapter.

The filesystem adapter exists so the application and benchmark can run without external services.

### S3 adapter

- Read from the configured bucket and region with the standard SDK credential provider chain.
- Never read credentials from the service TOML or expose them in logs.
- Support an optional `endpoint_url` and `force_path_style` for local S3-compatible test servers.
- Build the object key from the configured Key Prefix and validated source path.
- The request `v` parameter is a cache token. Do not send it as an S3 `versionId`.
- Treat a modeled missing-key response or upstream 404 as Source Object absence.
- Treat access denial, including 403, as Source unavailability rather than absence.
- Map SDK or transport timeouts to the same timeout outcome as HTTP(S).
- Check the reported object length and enforce the byte limit again while streaming.
- Document that an S3-compatible deployment may need bucket-list permission for the object store to
  distinguish a missing key from access denial. The adapter must never turn a denial into 404.

S3 tests use a local S3-compatible fixture or protocol-level fake. They must not require a cloud
account or public network access.

### Optional additional adapters

Other object stores may be supported as separate adapters. They must preserve the same error
taxonomy. In particular, permission denial is Source unavailability, not absence.

## Image processing

For an accepted request:

1. Acquire one of `max_concurrent_derivations` process-wide permits before fetching. No more than
   that number of Source Objects may be fetched or processed at once.
2. Fetch the Source Object with the selected adapter.
3. Reject it if its downloaded bytes exceed the configured limit.
4. Read image metadata. Accept JPEG, PNG, WebP, and AVIF raster sources only.
5. Reject animated or multi-page input. SVG, PDF, and other document loaders are not accepted.
6. Apply EXIF orientation, then calculate the oriented width and height.
7. Convert dimensions to unsigned 64-bit integers with checked arithmetic. Reject the source when
   `width * height > max_source_megapixels * 1_000_000`.
8. Decode the source.
9. Resize to the requested width while preserving aspect ratio.
10. If the source is narrower than the requested width, keep its original dimensions.
11. Preserve transparency for WebP and AVIF.
12. Flatten transparency onto white before JPEG encoding.
13. Encode with the selected format policy's requested or default quality and strip source
    metadata from the output.

The process-wide libvips runtime must be initialized once. Code must not clone or double-release
native image handles.

The build environment must contain a libvips version that supports every enabled encoder. AVIF
support must be verified at runtime or by an executable test, not inferred from a successful
compile.

## Responses

### Success

A successful response returns 200 with:

```text
Content-Type: image/webp | image/avif | image/jpeg
X-Content-Type-Options: nosniff
Content-Security-Policy: default-src 'none'
X-Frame-Options: DENY
```

The body is the encoded Derived Image.

A request with a non-empty `v` returns:

```text
Cache-Control: public, max-age=31536000, immutable
```

An unversioned request returns:

```text
Cache-Control: public, max-age={unversioned_success_ttl_seconds}
```

An unversioned response must not include `immutable`.

### Errors

Every error response is JSON:

```json
{ "error": "stable public message" }
```

It has `Content-Type: application/json`. The evaluator checks that `error` is a non-empty string
and does not compare its exact wording.

Use this status taxonomy:

- 400 for an invalid path, Mount, query, Transform, width, format, or quality.
- 404 when the Source Object is absent.
- 405 for an unsupported HTTP method.
- 500 when resize, flatten, or encode fails after a valid source image was accepted.
- 502 when the Source answers but does not provide a usable object, including permission errors,
  non-absence upstream statuses, oversized content, and undecodable image bytes.
- 504 when fetching the Source times out.

A 404 carries:

```text
Cache-Control: public, max-age={not_found_ttl_seconds}
```

Every error other than 404, including 400, 405, and all 5xx responses, carries:

```text
Cache-Control: no-store
```

A transient failure or method error must not be pinned by an external cache.

## Observability

Write structured JSON logs to standard output.

Emit one completion event per request with:

- HTTP status
- stable low-cardinality outcome
- Mount
- output width and format when parsed
- upstream status when one exists
- input and output byte counts when known
- elapsed milliseconds

Use a closed outcome set:

```text
success
rejected_request
not_found
timeout
source_too_large
source_unavailable
undecodable_source
resize_failed
flatten_failed
encode_failed
```

Do not log response bodies, credentials, or the value of the source-version query parameter.

## Recommended module shape

This section is non-normative and is not scored. Keep the main interface small:

```text
HTTP request
  -> request parser and policy
  -> Source registry
  -> selected Source adapter
  -> image processor
  -> HTTP response
```

The request parser should return a resolved request containing a Source reference, upstream key,
and Transform. The rest of the application should not parse URL strings again.

The image processor accepts bytes plus a resolved Transform and returns bytes or a typed processing
error. It does not know about HTTP, Sources, or configuration allowlists.

Keep request errors, Source errors, and processing errors as separate types. Their variants encode
the status taxonomy rather than relying on message matching.

## Test design and mutation resilience

Tests specify observable behavior, not the reference implementation's control flow. A conforming
implementation may reorganize modules, choose different private types, or replace an algorithm
without rewriting the contract tests.

Tests must:

- drive pure policy through public module interfaces and integrations through HTTP
- assert statuses, headers, resolved keys, emitted image properties, limits, and Source fixture
  observations
- avoid private function names, internal call order, exact log prose, line coverage targets, and
  snapshots of implementation-owned debug output
- use fixture-server call counts only when the contract says a rejected request must perform no
  Source I/O
- parameterize values and include generated cases so passing does not depend on copying examples
  from this document
- compare lossy images by documented properties and tolerances, not exact bytes

The benchmark release must run mutation testing with `cargo-mutants` or an equivalent Rust mutation
tool. Generated code, process bootstrap, and thin native-library bindings may be excluded with a
written reason. The remaining code must reach at least a 90 percent mutation score.

The evaluator also maintains a fixed semantic mutation suite. Every release must prove that its
tests kill each of these changes:

- accept a width outside the Width Allowlist
- consult one global quality allowlist instead of the selected format policy
- accept an explicit quality equal to the selected format's default
- require `v`, or mark an unversioned response immutable
- append `v` to an HTTP(S) path or S3 object key
- permit upscaling by reversing the width comparison
- map Source denial to 404
- cache a 5xx response
- remove the streamed-body size check while keeping the `Content-Length` check
- remove encoded traversal validation
- allow a redirect outside the configured origin or base path
- allow a filesystem symlink
- flatten alpha for WebP or fail to flatten it for JPEG

A surviving semantic mutant is a benchmark defect even if line or branch coverage is 100 percent.
Equivalent mutants must be documented with the reason they cannot change observable behavior.

## Public test suite

The repository must include tests for the following behavior.

### Request and policy tests

- A valid nested source path resolves to the expected Mount, upstream key, and Transform.
- A Key Prefix may differ from the Mount or be empty.
- Dots in filenames and directories survive unchanged.
- Missing `v` is accepted.
- Empty, overlong, percent-encoded, or non-canonical `v` values return 400.
- Repeated `v` and unknown query parameters return 400.
- Non-canonical percent encoding and a request target over 8192 bytes return 400.
- Unknown Mounts return 400 without contacting any Source.
- Widths, unconfigured formats, and qualities outside the selected format policy return 400 without
  a fetch.
- Quality is closed independently for each format whose allowlist is empty.
- A quality allowed for one format remains invalid for another unless both policies list it.
- Quality equal to a format default is rejected.
- Field reordering, repetition, aliases, signs, and leading zeros are rejected.
- Missing source path or Transform is rejected.
- Literal and encoded traversal attempts are rejected.

### Source adapter tests

- HTTP(S) 404 and 410 map to absence.
- HTTP(S) 403 and 500 map to unavailability.
- HTTPS succeeds with the configured test CA and still enforces hostname verification.
- Connection and body timeouts map to timeout.
- Redirect count, same-origin, and base-path restrictions are enforced.
- Encoded HTTP response bodies are rejected.
- Advertised and streamed byte limits are both enforced.
- Filesystem reads reject every symlink below the configured root.
- S3 missing-key and 404 responses map to absence.
- S3 access denial maps to unavailability, never absence.
- S3 keys exclude `v`, and S3 bodies enforce both advertised and streamed byte limits.

Use local HTTP and S3-compatible fixture servers. Tests must not call the public internet.

### Image tests

Generate image fixtures during the test run where practical.

- A 2000 by 1000 source requested at width 500 becomes 500 by 250.
- A 400-pixel source requested at width 1920 remains 400 pixels wide.
- JPEG, WebP, and AVIF outputs have the requested dimensions and container format.
- A transparent source becomes white when encoded as JPEG.
- Transparency survives WebP and AVIF encoding.
- A source above the megapixel limit is rejected.
- Checked dimension arithmetic cannot overflow.
- Animated, multi-page, SVG, PDF, and unsupported raster input is rejected.
- EXIF orientation is applied before resize and source metadata is absent from the output.
- Invalid image bytes map to an undecodable-source 502.

Do not compare lossy output byte for byte. Assert container signatures, dimensions, alpha behavior,
and pixel values with a documented tolerance.

### Response and error tests

- Versioned successful responses have the required content type, immutable cache policy, and
  security headers.
- Unversioned successful responses use the shorter configured TTL and are not immutable.
- Absence is the only Source outcome that returns a non-5xx status.
- Only 404 errors are cacheable.
- Every non-404 error returns `Cache-Control: no-store`.
- Permission denial cannot be mistaken for absence.
- Invalid JSON characters in an internal message cannot corrupt the public error body.
- Processing failures distinguish invalid source bytes from failures in the service's own pipeline.

### Configuration tests

- Every invalid configuration listed above fails startup with a useful message.
- The example configuration parses.
- A missing per-format quality allowlist permits no explicit quality for that format.
- Different formats may configure different defaults and quality allowlists.
- Duplicate Mounts cannot silently replace one another.

## End-to-end acceptance

The evaluator starts a local fixture server and the Image Service in separate processes. It then
verifies:

1. A source JPEG can be fetched and returned as a smaller WebP.
2. The same source can be returned as AVIF and JPEG.
3. The output dimensions preserve aspect ratio.
4. A source narrower than the target is not enlarged.
5. A missing source returns a cacheable 404.
6. A denied source returns a non-cacheable 502.
7. A slow source returns a non-cacheable 504.
8. A streamed response over the byte limit is stopped.
9. Invalid requests do not reach the fixture server.
10. Traversal attempts cannot read a filesystem sentinel placed outside the configured root.
11. The container starts from the example configuration and passes the same success case.
12. Omitting `v` still returns the Derived Image, but with the shorter non-immutable cache policy.
13. Concurrent fixture-server requests never exceed `max_concurrent_derivations`.
14. The same source-path contract works through a local S3-compatible Source.
15. An HTTPS Source works through a local TLS fixture configured with its test CA.

## Benchmark packaging

The benchmark repository should contain:

- this specification as `SPEC.md`
- a compiling Rust scaffold with domain types and function stubs
- public fixtures or fixture generators
- public tests for representative behavior
- a hidden test package maintained outside the candidate checkout
- a semantic-mutant manifest and reproducible mutation-testing command
- a Docker-based evaluator that installs the exact native image dependencies
- one command that runs formatting, linting, unit tests, and integration tests

The candidate must not need secrets or network access beyond loopback. The benchmark release owns
and publishes:

- an exact `rust-toolchain.toml`
- a committed `Cargo.lock`
- an evaluator image referenced by digest
- pre-fetched Rust dependencies in that evaluator image
- the libvips and encoder versions reported during the run
- image fixtures and numeric pixel tolerances

After the evaluator image is built, candidate execution has no public-network access. Evaluate from
a clean checkout.

Hidden tests should vary Mount names, prefixes, width allowlists, per-format defaults and quality
allowlists, paths, image dimensions, upstream statuses, transfer encodings, and redirect behavior.
They should test the published contract, not private function names or a particular internal file
layout.

Treat these as hard failures regardless of the numeric score:

- arbitrary URL fetching
- filesystem escape
- acceptance of a width or quality outside its allowlist
- failure to enforce streamed byte limits
- upscaling
- reporting permission denial as 404
- an OCI image that cannot encode every format enabled by the example configuration

Suggested scoring:

- 25 points for request parsing, canonicalization, and configuration.
- 20 points for Source routing, adapters, and error classification.
- 20 points for image correctness.
- 15 points for security and resource limits.
- 10 points for HTTP responses, caching, and observability.
- 5 points for mutation-resistant tests added by the candidate.
- 5 points for container reproducibility and documentation.

### Benchmark scope

This document defines one benchmark profile: the complete specification above. A shorter benchmark
may be derived from it, but it needs its own `SPEC.md`, acceptance tests, hard-failure list, and
scoring weights. Scores from reduced and complete profiles are not comparable.

## Suggested implementation plan

This sequence is guidance for project authors. The evaluator scores the resulting behavior, not the
order of implementation.

### Phase 1: contract and pure policy

Create domain types, configuration parsing, Source registry, path validation, Transform parsing,
and typed request errors. Complete request and configuration tests before adding network or image
dependencies.

### Phase 2: Source adapters

Add the internal Source interface, then filesystem, HTTP(S), and S3 adapters. Use local fixtures to
lock down redirects, timeouts, absence, denial, and byte limits.

### Phase 3: image processor

Initialize libvips once, add metadata limits, downscale-only resizing, alpha handling, and the three
encoders. Add executable format tests, especially for AVIF.

### Phase 4: HTTP application

Compose parsing, fetching, processing, response headers, JSON errors, and structured completion
logs. Add end-to-end tests through a real listening socket.

### Phase 5: distribution

Add the container image, pinned toolchain, reproducible CI command, example configuration, license,
security policy, and operator documentation.

### Phase 6: benchmark extraction

Move selected tests to the hidden evaluator, replace implementation bodies with a compiling
scaffold, and run the benchmark against at least one known-good and several deliberately broken
implementations. Confirm that each hard-failure condition is caught by one focused test.

## Deliberate non-goals

- uploads or image management
- arbitrary remote URLs supplied by callers
- cropping, gravity, rotation, height, device-pixel ratio, or free-form quality
- user accounts, tenancy, or per-caller policy
- a built-in persistent cache
- CDN or cloud provisioning
- client UI integration
- migration from another image product
- source-version generation or cache invalidation
- exact byte reproducibility across encoder versions

These can become separate projects. Adding them to the benchmark would test integration volume
rather than the image service's core design.

## Open-source completion criteria

This section is a publication checklist, not part of hidden-test scoring. The standalone project is
ready to publish when:

- its tracked files contain no private hostnames, account identifiers, organization names, or
  product-specific examples
- all examples use reserved domains such as `example.test`
- the full test suite runs without credentials and without public-network access
- the container build pins or verifies the native encoder capabilities it enables
- an OSI-approved license and third-party notices are present
- `SECURITY.md` explains how to report path traversal, SSRF, decompression bomb, and native-library
  issues
- the README documents the URL contract, configuration, limits, error taxonomy, local run command,
  container run command, and S3 credential and permission requirements
- a fresh contributor can run the service against bundled fixtures in one documented command
