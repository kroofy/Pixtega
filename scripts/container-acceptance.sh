#!/usr/bin/env bash
# End-to-end acceptance item 11 (numbering follows tests/e2e_tests.rs):
# both container images start, derive every supported format, and stop
# cleanly.
#
# The images carry their own libvips build (see the Dockerfile), not the one
# the test suite runs against, so this script is what proves that build can
# do everything the service promises. For the service image (baked-in
# example configuration) and the Lambda image (configuration inline through
# CONFIG, as on Lambda), it:
#
#   - runs the image on an ephemeral loopback port and waits for the
#     service's "listening" startup line;
#   - derives WebP, AVIF, and JPEG from each of a JPEG, a PNG, a WebP, and
#     an AVIF source, asserting a 200, the Content-Type, and the container
#     signature of all twelve bodies;
#   - stops the container and asserts it exited 0 on SIGTERM instead of
#     being killed when the grace period ran out.
#
# Requires Docker and curl. Exits non-zero on any failure.
set -euo pipefail
cd "$(dirname "$0")/.."

SERVICE_IMAGE="pixtega-acceptance:local"
LAMBDA_IMAGE="pixtega-acceptance:lambda"
CONTAINER_NAME="pixtega-acceptance-$$"
WORK_DIR="$(mktemp -d)"

cleanup() {
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

fail() {
    echo "FAIL: $*" >&2
    docker logs "$CONTAINER_NAME" >&2 2>/dev/null || true
    exit 1
}

# Hex of $3 bytes of file $1 starting at offset $2.
hex_at() {
    dd if="$1" bs=1 skip="$2" count="$3" 2>/dev/null | od -An -tx1 | tr -d ' \n'
}

# GET $BASE_URL$1 into file $2 and assert a 200 in format $3.
derive() {
    local target="${BASE_URL}$1" body="$2" format="$3"
    local headers="${WORK_DIR}/headers.txt" status signature
    status="$(curl -sS -o "$body" -D "$headers" -w '%{http_code}' "$target")"
    if [ "$status" != "200" ]; then
        cat "$headers" >&2 || true
        fail "expected HTTP 200 from ${target}, got ${status}"
    fi
    if ! grep -qi "^content-type: *image/${format}" "$headers"; then
        cat "$headers" >&2
        fail "expected Content-Type: image/${format} from ${target}"
    fi
    case "$format" in
        webp) signature="$(hex_at "$body" 0 4)$(hex_at "$body" 8 4)" ;; # RIFF....WEBP
        avif) signature="$(hex_at "$body" 4 8)" ;;                      # ....ftypavif
        jpeg) signature="$(hex_at "$body" 0 3)" ;;                      # SOI + marker
    esac
    case "${format}:${signature}" in
        webp:5249464657454250 | avif:6674797061766966 | jpeg:ffd8ff) ;;
        *) fail "body of ${target} is not ${format} (signature ${signature})" ;;
    esac
    echo "    OK $1 ($(wc -c <"$body") bytes)"
}

# accept IMAGE [docker run arguments...]
accept() {
    local image="$1"
    shift
    echo "==> ${image}"

    # Publish container port 8080 on an ephemeral loopback port so the host
    # port is never taken.
    docker run -d --name "$CONTAINER_NAME" -p 127.0.0.1:0:8080 "$@" "$image" >/dev/null
    local host_port
    host_port="$(docker port "$CONTAINER_NAME" 8080/tcp | head -n1 | sed 's/.*://')"
    [ -n "$host_port" ] || fail "could not determine the published host port"
    BASE_URL="http://127.0.0.1:${host_port}"

    for _ in $(seq 1 60); do
        if docker logs "$CONTAINER_NAME" 2>/dev/null | grep -q '"event":"listening"'; then
            break
        fi
        if [ -z "$(docker ps -q --filter "name=^${CONTAINER_NAME}$")" ]; then
            fail "container exited during startup"
        fi
        sleep 1
    done
    docker logs "$CONTAINER_NAME" 2>/dev/null | grep -q '"event":"listening"' \
        || fail "service never announced listening"

    # Every output format from every source format. The JPEG source is the
    # fixture bundled in the image. The WebP and AVIF sources are what the
    # service derives from it, and the PNG is one transparent pixel, which
    # also takes the flatten-onto-white path when derived to JPEG.
    local fixture="/images/fixtures/photos/example.6acca6c1.jpg"
    local sources="${WORK_DIR}/acceptance"
    local source format
    rm -rf "$sources"
    mkdir "$sources"
    for format in webp avif jpeg; do
        derive "${fixture}/w640.${format}" "${sources}/photo.${format}" "$format"
    done
    base64 -d >"${sources}/pixel.png" <<'PNG'
iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==
PNG
    docker cp -q "$sources" "${CONTAINER_NAME}:/app/fixtures/acceptance"
    for source in pixel.png photo.webp photo.avif; do
        for format in webp avif jpeg; do
            derive "/images/fixtures/acceptance/${source}/w320.${format}" "${WORK_DIR}/out" "$format"
        done
    done

    # The service is PID 1 in the container, where an unhandled SIGTERM is
    # ignored: `docker stop` would then wait out its grace period and kill.
    docker stop "$CONTAINER_NAME" >/dev/null
    local exit_code
    exit_code="$(docker inspect -f '{{.State.ExitCode}}' "$CONTAINER_NAME")"
    [ "$exit_code" = "0" ] || fail "expected a clean exit on SIGTERM, got exit code ${exit_code}"
    echo "    OK stopped cleanly on SIGTERM"
    docker rm "$CONTAINER_NAME" >/dev/null
}

# CI pre-builds both images with buildx + GitHub Actions layer caching and
# sets PIXTEGA_SKIP_BUILD=1; everywhere else the script builds them here.
if [ "${PIXTEGA_SKIP_BUILD:-0}" = "1" ]; then
    echo "==> skipping docker build (PIXTEGA_SKIP_BUILD=1), using existing images"
    docker image inspect "$SERVICE_IMAGE" "$LAMBDA_IMAGE" >/dev/null
else
    echo "==> docker build"
    docker build -t "$SERVICE_IMAGE" .
    docker build --target lambda -t "$LAMBDA_IMAGE" .
fi

# The service image's default CMD is /app/config.example.toml. The Lambda
# image has no default configuration and takes it inline, as on Lambda.
accept "$SERVICE_IMAGE"
accept "$LAMBDA_IMAGE" -e CONFIG="$(cat deploy/lambda/example-config.toml)"

echo "OK: both images derive WebP, AVIF, and JPEG from JPEG, PNG, WebP, and AVIF sources"
