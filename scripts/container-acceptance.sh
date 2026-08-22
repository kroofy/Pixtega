#!/usr/bin/env bash
# End-to-end acceptance item 11 (numbering follows tests/e2e_tests.rs):
# the container image starts from the
# baked-in example configuration and passes the fixtures-mount success case.
#
# Builds the OCI image from the repository Dockerfile, runs it on an
# ephemeral loopback port, waits for the service's "listening" startup line,
# then requests /images/fixtures/photos/example.jpg/w640.webp and asserts a
# 200 response with Content-Type: image/webp and a WebP (RIFF) body.
#
# Requires Docker and curl. Exits non-zero on any failure.
set -euo pipefail
cd "$(dirname "$0")/.."

IMAGE_TAG="pixtega-acceptance:local"
CONTAINER_NAME="pixtega-acceptance-$$"
WORK_DIR="$(mktemp -d)"

cleanup() {
    docker rm -f "$CONTAINER_NAME" >/dev/null 2>&1 || true
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

echo "==> docker build"
docker build -t "$IMAGE_TAG" .

echo "==> docker run (example configuration baked into the image)"
# The image's default CMD is /app/config.example.toml; publish container
# port 8080 on an ephemeral loopback port so the host port is never taken.
docker run -d --name "$CONTAINER_NAME" -p 127.0.0.1:0:8080 "$IMAGE_TAG" >/dev/null

HOST_PORT="$(docker port "$CONTAINER_NAME" 8080/tcp | head -n1 | sed 's/.*://')"
if [ -z "$HOST_PORT" ]; then
    echo "FAIL: could not determine the published host port" >&2
    exit 1
fi
BASE_URL="http://127.0.0.1:${HOST_PORT}"

echo "==> waiting for the service startup line"
for _ in $(seq 1 60); do
    if docker logs "$CONTAINER_NAME" 2>/dev/null | grep -q '"event":"listening"'; then
        break
    fi
    if [ -z "$(docker ps -q --filter "name=^${CONTAINER_NAME}$")" ]; then
        echo "FAIL: container exited during startup; logs:" >&2
        docker logs "$CONTAINER_NAME" >&2 || true
        exit 1
    fi
    sleep 1
done
if ! docker logs "$CONTAINER_NAME" 2>/dev/null | grep -q '"event":"listening"'; then
    echo "FAIL: service never announced listening; logs:" >&2
    docker logs "$CONTAINER_NAME" >&2 || true
    exit 1
fi

echo "==> requesting the fixtures-mount success case"
TARGET="${BASE_URL}/images/fixtures/photos/example.jpg/w640.webp"
BODY_FILE="${WORK_DIR}/derived.webp"
HEADERS_FILE="${WORK_DIR}/headers.txt"
STATUS="$(curl -sS -o "$BODY_FILE" -D "$HEADERS_FILE" -w '%{http_code}' "$TARGET")"

if [ "$STATUS" != "200" ]; then
    echo "FAIL: expected HTTP 200 from ${TARGET}, got ${STATUS}" >&2
    cat "$HEADERS_FILE" >&2 || true
    exit 1
fi

if ! grep -qi '^content-type: *image/webp' "$HEADERS_FILE"; then
    echo "FAIL: expected Content-Type: image/webp; response headers:" >&2
    cat "$HEADERS_FILE" >&2
    exit 1
fi

# WebP container signature: bytes 0-3 "RIFF", bytes 8-11 "WEBP".
if [ "$(head -c 4 "$BODY_FILE")" != "RIFF" ] \
    || [ "$(dd if="$BODY_FILE" bs=1 skip=8 count=4 2>/dev/null)" != "WEBP" ]; then
    echo "FAIL: response body is not a WebP container" >&2
    exit 1
fi

echo "OK: container serves ${TARGET} as image/webp ($(wc -c <"$BODY_FILE") bytes)"
