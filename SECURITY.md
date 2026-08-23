# Security policy

## Reporting a vulnerability

Report security issues privately through the repository's GitHub Security
Advisories ("Report a vulnerability"). Do not open a public issue for a
vulnerability. You should receive an acknowledgement within a few days.

Please include a minimal reproduction: the configuration file, the exact
request target (URL-encoded form), and the observed versus expected
behavior.

## What qualifies

This service's security model makes the following bug classes especially
important. All are in scope even when they require an unusual configuration.

### Path traversal

A request must never read outside the configured Source. Relevant surfaces:

- the source-path validator (`src/request.rs`): literal, percent-encoded,
  and double-encoded `.`/`..`, encoded `/` and `\`, non-canonical percent
  encoding
- the filesystem adapter (`src/sources/filesystem.rs`): symlinks below the
  root, non-regular files
- the Key Prefix validation in configuration loading

If you can make the service return the contents of a file or URL outside a
configured Source root, bucket, or base URL, that is a critical report.

### Server-side request forgery (SSRF)

Callers must never influence which host the service talks to. The Mount is
the only routing input, and redirects from an HTTP(S) Source must stay on
the configured scheme, host, port, and base path. Additionally, private
and local destinations (loopback, link-local/metadata addresses, RFC 1918,
`localhost`, `*.internal`) are refused at startup and before every
connection unless a source explicitly sets `allow_private_destinations`.
A redirect or URL-construction bug that lets a response steer the service
to another origin — or to a blocked destination — is a critical report.

### Decompression bombs and resource exhaustion

The service enforces `max_download_bytes` (advertised and streamed),
`max_source_megapixels` (checked before full decode), a closed width
allowlist, rejection of animated/multi-page/document input, and
`max_concurrent_derivations`. A crafted input that bypasses any of these
limits — for example an image whose header understates its decoded size, or
a request pattern that exceeds the derivation permit count — is in scope.

### Native-library issues

Image decoding and encoding use libvips and its delegate libraries
(libjpeg, libpng, libwebp, libheif/aom). Crashes, memory unsafety, or hangs
triggered through this service's accepted input formats should be reported
here as well as upstream; include the input bytes. The container image pins
the distribution's libvips packages, and startup verifies each enabled
encoder, so fixes can be shipped by rebuilding the image.

## Non-goals

Missing authentication is not a vulnerability: the service is designed to
sit behind a CDN or reverse proxy and serves only derived images from
operator-configured Sources. Denial of service by sheer request volume is
out of scope; concurrency and size limits bound per-request cost, not
aggregate traffic.
