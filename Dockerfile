# One Dockerfile, two images:
#
#   docker build -t pixtega .                         # service image (default)
#   docker build --target lambda -t pixtega-lambda .  # + AWS Lambda Web Adapter
#
# Both are Ubuntu 26.04 (resolute) with a libvips built from source for
# exactly what the service does: JPEG, PNG, WebP, and AVIF in; WebP, AVIF,
# and JPEG out. The distribution's libvips42t64 links every codec libvips
# knows (ImageMagick, Poppler, OpenSlide, librsvg, OpenEXR, HDF5, ...): 135
# shared libraries mapped, and 62 MB read from disk to start and answer one
# request. This build maps 34 and reads 20 MB, which is what a cold start
# pays for (measure with scripts/bench/coldstart.sh). Loaders the service
# rejects anyway are not in the image at all.
#
# The codec libraries themselves (libjpeg-turbo, libpng, libwebp, libheif
# with the aom AV1 plugins, lcms2, libexif, highway) still come from Ubuntu
# and take its security updates on every rebuild. libvips is pinned here
# by version and checksum; see docs/ci.md for how it is bumped.
#
# The service verifies every enabled encoder at startup, so an image missing
# an encoder fails fast instead of serving partial formats.

FROM ubuntu:26.04 AS build

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        libexif-dev \
        libexpat1-dev \
        libglib2.0-dev \
        libheif-dev \
        libhwy-dev \
        libjpeg-turbo8-dev \
        liblcms2-dev \
        libpng-dev \
        libwebp-dev \
        meson \
        ninja-build \
        pkg-config \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Must stay on the minor version the pinned libvips Rust bindings were
# generated from (libvips = "=2.3.0" -> 8.18).
ARG VIPS_VERSION=8.18.7
ARG VIPS_SHA256=5baaead3b0bb20ffdb9e9ff09aa9fda08620923df77b63b436654cb5e0b3bf94
# auto_features=disabled turns every optional dependency off; the -D...=enabled
# list is then the complete set of what this libvips can do. The built-in
# GIF, PPM, Analyze, and Radiance loaders are plain booleans and are turned
# off by name.
RUN curl --proto '=https' --tlsv1.2 -fsSL -o /tmp/vips.tar.xz \
        "https://github.com/libvips/libvips/releases/download/v${VIPS_VERSION}/vips-${VIPS_VERSION}.tar.xz" \
    && echo "${VIPS_SHA256}  /tmp/vips.tar.xz" | sha256sum -c - \
    && mkdir /tmp/vips \
    && tar -xJf /tmp/vips.tar.xz -C /tmp/vips --strip-components=1 \
    && meson setup /tmp/vips/build /tmp/vips \
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
        -Dradiance=false \
    && meson install -C /tmp/vips/build \
    && install -D -m 644 /tmp/vips/LICENSE /usr/local/share/doc/libvips/LICENSE \
    && ldconfig \
    && rm -rf /tmp/vips /tmp/vips.tar.xz

# LIBRARY_PATH: the libvips bindings link with a bare -lvips, and the Rust
# toolchain's linker does not search /usr/local/lib on its own.
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    LIBRARY_PATH=/usr/local/lib
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain none

WORKDIR /app
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
# rustup installs the toolchain pinned by rust-toolchain.toml on first use.
RUN rustup show active-toolchain || rustup toolchain install

# Compile dependencies against stub sources so they land in their own layer,
# keyed on Cargo.lock alone. Source-only changes then rebuild just this crate.
RUN mkdir src \
    && echo '' > src/lib.rs \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
# touch: the stub build already produced crate artifacts with newer mtimes
# than the COPYed sources, so without it cargo would skip recompiling and
# ship the stub binary.
RUN touch src/lib.rs src/main.rs && cargo build --release --locked

# A named stage so Dependabot's docker ecosystem sees the adapter pin.
FROM public.ecr.aws/awsguru/aws-lambda-adapter:1.0.1 AS lambda-adapter

# Everything both images share. The package list is the runtime half of the
# -dev packages libvips was built against above.
FROM ubuntu:26.04 AS base

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libexif12 \
        libexpat1 \
        libglib2.0-0t64 \
        libheif-plugin-aomdec \
        libheif-plugin-aomenc \
        libheif1 \
        libhwy1t64 \
        libjpeg-turbo8 \
        liblcms2-2 \
        libpng16-16t64 \
        libwebp7 \
        libwebpdemux2 \
        libwebpmux3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --shell /usr/sbin/nologin pixtega

# The glob takes the libvips.so.42 soname symlink and the file it names.
COPY --from=build /usr/local/lib/libvips.so.42* /usr/local/lib/
COPY --from=build /usr/local/share/doc/libvips /usr/local/share/doc/libvips
COPY --from=build /app/target/release/pixtega /usr/local/bin/pixtega
# Fail the build, not the first start, if a library the binary needs is
# missing from the package list above.
RUN ldconfig && ! ldd /usr/local/bin/pixtega | grep "not found"

WORKDIR /app
COPY fixtures ./fixtures

USER pixtega
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/pixtega"]

# AWS Lambda image: the service image plus the AWS Lambda Web Adapter
# extension, which translates Lambda invocations (Function URL, API Gateway,
# ALB) into plain HTTP against the service. It still runs anywhere a normal
# container runs. See deploy/lambda/README.md for the deployment walkthrough.
FROM base AS lambda

COPY --from=lambda-adapter /lambda-adapter /opt/extensions/lambda-adapter
# The adapter forwards to 127.0.0.1:8080 by default (AWS_LWA_PORT), so
# configure listen_address = "0.0.0.0:8080".
#
# AWS_LWA_READINESS_CHECK_PROTOCOL=tcp: the service has no HTTP health
# endpoint (GET / is a 400 by design), so the adapter waits for the TCP
# socket instead of an HTTP 200.
#
# AWS_LWA_INVOKE_MODE=response_stream: the Function URL is expected to use
# InvokeMode RESPONSE_STREAM (image bodies can exceed the ~6 MB buffered
# response cap), and the adapter's invoke mode must match the Function
# URL's — a mismatch returns the adapter's buffered JSON envelope instead of
# the image bytes. ALB cannot stream, and API Gateway streams only via a
# STREAM-mode REST proxy integration: otherwise override to
# AWS_LWA_INVOKE_MODE=buffered.
ENV AWS_LWA_READINESS_CHECK_PROTOCOL=tcp \
    AWS_LWA_INVOKE_MODE=response_stream
# No default config: with no CLI argument the service falls back to
# CONFIG_FILE and then CONFIG, so the full TOML can be supplied inline
# through the function's CONFIG environment variable without a rebuild.
CMD []

# The service image. Last, so a plain `docker build .` produces it.
FROM base AS service

COPY config.example.toml ./config.example.toml
CMD ["/app/config.example.toml"]
