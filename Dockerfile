# syntax=docker/dockerfile:1

# ---------------------------------------------------------------------------
# 1. The Leptos frontend, compiled to wasm and bundled by trunk.
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS frontend

ARG TRUNK_VERSION=0.21.14
RUN set -eux; \
    case "$(uname -m)" in \
      x86_64) trunk_arch=x86_64-unknown-linux-gnu ;; \
      aarch64) trunk_arch=aarch64-unknown-linux-gnu ;; \
      *) echo "unsupported build host: $(uname -m)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-${trunk_arch}.tar.gz" \
      | tar -xzf - -C /usr/local/bin trunk; \
    rustup target add wasm32-unknown-unknown

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY assets ./assets

# Writes to /app/dist/web, per crates/frontend/Trunk.toml.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    trunk build --release --config crates/frontend/Trunk.toml

# ---------------------------------------------------------------------------
# 2. The Axum backend (bot + web server) as a static musl binary.
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS backend

# musl gives a statically linked binary. Together with the `rustls` feature in
# crates/backend/Cargo.toml that removes every runtime dependency, libssl included.
RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    set -eux; \
    case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-musl ;; \
      arm64) triple=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    rustup target add "$triple"; \
    cargo build --release --target "$triple" --package backend; \
    # /app/target is a build cache mount, so copy the binary out of it.
    cp "target/$triple/release/telegram-loader" /telegram-loader

# ---------------------------------------------------------------------------
# 3. Runtime: the binary is static, and so is the ffmpeg that makes the video
#    thumbnails, so nothing else is needed.
# ---------------------------------------------------------------------------
FROM scratch
COPY --from=backend /telegram-loader /telegram-loader
COPY --from=mwader/static-ffmpeg:7.1 /ffmpeg /ffmpeg
COPY --from=frontend /app/dist/web /app/web

# SAVE_DIR and THUMB_DIR are relative to the working directory, so media lands
# in /data/downloads and thumbnails in /data/thumbnails.
WORKDIR /data
VOLUME /data
ENV RUST_LOG=info \
    FRONTEND_DIR=/app/web \
    FFMPEG_BIN=/ffmpeg \
    BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/telegram-loader"]
