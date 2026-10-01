FROM rust:1.92-bookworm AS builder

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked -p parking-ingestion-worker && \
    cp target/release/parking-ingestion-worker /usr/local/bin/parking-ingestion-worker

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --system --gid 10001 appgroup \
    && useradd \
        --system \
        --uid 10001 \
        --gid appgroup \
        --no-create-home \
        --shell /usr/sbin/nologin \
        appuser

COPY --from=builder /usr/local/bin/parking-ingestion-worker /usr/local/bin/parking-ingestion-worker

USER appuser

ENTRYPOINT ["/usr/local/bin/parking-ingestion-worker"]