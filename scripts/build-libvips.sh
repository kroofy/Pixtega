#!/usr/bin/env bash
# Build and install the libvips the container images ship: the pinned
# upstream release with exactly what the service uses (JPEG, PNG, WebP, and
# AVIF in; WebP, AVIF, and JPEG out) and nothing else.
#
# One recipe, two callers: the Dockerfile's build stage, and the CI check
# job, so the test suite runs against the same libvips the images carry.
#
# Run as root on Ubuntu 26.04. Installs the build dependencies with apt,
# installs libvips into /usr/local, and runs ldconfig. To link against it,
# cargo needs LIBRARY_PATH=/usr/local/lib: the libvips bindings link with a
# bare -lvips, and the Rust toolchain's linker does not search /usr/local/lib
# on its own.
#
# The codec libraries stay Ubuntu packages and take Ubuntu's security
# updates. libvips itself is pinned here; docs/ci.md says how to bump it.
set -euo pipefail

# Must stay on the minor version the pinned libvips Rust bindings were
# generated from (libvips = "=2.3.0" -> 8.18).
VIPS_VERSION=8.18.7
VIPS_SHA256=5baaead3b0bb20ffdb9e9ff09aa9fda08620923df77b63b436654cb5e0b3bf94

# The -dev packages are what libvips is built against; their runtime halves
# are the package list of the Dockerfile's `base` stage. The libheif plugins
# are loaded at run time and carry the AV1 codec, so the build can be tested
# where it was made.
apt-get update
apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    curl \
    libexif-dev \
    libexpat1-dev \
    libglib2.0-dev \
    libheif-dev \
    libheif-plugin-aomdec \
    libheif-plugin-aomenc \
    libhwy-dev \
    libjpeg-turbo8-dev \
    liblcms2-dev \
    libpng-dev \
    libwebp-dev \
    meson \
    ninja-build \
    pkg-config \
    xz-utils

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl --proto '=https' --tlsv1.2 -fsSL -o "$work/vips.tar.xz" \
    "https://github.com/libvips/libvips/releases/download/v${VIPS_VERSION}/vips-${VIPS_VERSION}.tar.xz"
echo "${VIPS_SHA256}  $work/vips.tar.xz" | sha256sum -c -
mkdir "$work/src"
tar -xJf "$work/vips.tar.xz" -C "$work/src" --strip-components=1

# auto_features=disabled turns every optional dependency off; the
# -D...=enabled list is then the complete set of what this libvips can do.
# The built-in GIF, PPM, Analyze, and Radiance loaders are plain booleans
# and are turned off by name.
meson setup "$work/build" "$work/src" \
    --prefix=/usr/local --libdir=lib --buildtype=release \
    -Dauto_features=disabled \
    -Dexif=enabled \
    -Dheif=enabled \
    -Dhighway=enabled \
    -Djpeg=enabled \
    -Dlcms=enabled \
    -Dpng=enabled \
    -Dwebp=enabled \
    -Danalyze=false \
    -Dcplusplus=false \
    -Ddeprecated=false \
    -Dexamples=false \
    -Dmodules=disabled \
    -Dnsgif=false \
    -Dppm=false \
    -Dradiance=false
meson install -C "$work/build"
install -D -m 644 "$work/src/LICENSE" /usr/local/share/doc/libvips/LICENSE
ldconfig
