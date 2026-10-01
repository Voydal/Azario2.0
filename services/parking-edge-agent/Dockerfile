FROM rust:1.92-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       pkg-config libglib2.0-dev libgstreamer1.0-dev \
       libgstreamer-plugins-base1.0-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked -p parking-edge-agent \
    && install -m 0755 target/release/parking-edge-agent \
       /usr/local/bin/parking-edge-agent

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
       ca-certificates \
       libgstreamer1.0-0 libgstreamer-plugins-base1.0-0 \
       gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
       gstreamer1.0-libav \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 appgroup \
    && useradd --system --uid 10001 --gid appgroup \
       --no-create-home --shell /usr/sbin/nologin appuser \
    && mkdir -p /etc/parking-edge /var/lib/parking-edge \
    && chown appuser:appgroup /var/lib/parking-edge

COPY --from=builder /usr/local/bin/parking-edge-agent \
    /usr/local/bin/parking-edge-agent

USER appuser

WORKDIR /var/lib/parking-edge

ENTRYPOINT ["/usr/local/bin/parking-edge-agent"]
CMD ["--config", "/etc/parking-edge/camera.toml"]