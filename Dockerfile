# Build and runtime are both Ubuntu 24.04 (noble): its libvips 8.15 matches
# the pinned libvips Rust bindings (=1.6.1) and its libheif ships the aom
# AV1 encoder plugin needed for AVIF output. The service verifies every
# enabled encoder at startup, so an image missing an encoder fails fast
# instead of serving partial formats.

FROM ubuntu:24.04 AS build

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        libheif-plugin-aomdec \
        libheif-plugin-aomenc \
        libvips-dev \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain none

WORKDIR /app
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
# rustup installs the toolchain pinned by rust-toolchain.toml on first use.
RUN rustup show active-toolchain || rustup toolchain install

COPY src ./src
RUN cargo build --release --locked

FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libheif-plugin-aomdec \
        libheif-plugin-aomenc \
        libvips42t64 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --shell /usr/sbin/nologin pixtega

WORKDIR /app
COPY --from=build /app/target/release/pixtega /usr/local/bin/pixtega
COPY config.example.toml ./config.example.toml
COPY fixtures ./fixtures

USER pixtega
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/pixtega"]
CMD ["/app/config.example.toml"]
