FROM rust:1.92-bookworm AS builder

WORKDIR /app

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked -p parking-api && \
    cp target/release/parking-api /usr/local/bin/parking-api

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

COPY --from=builder /usr/local/bin/parking-api /usr/local/bin/parking-api

USER appuser

EXPOSE 3000

ENTRYPOINT ["/usr/local/bin/parking-api"]