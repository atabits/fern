# syntax=docker/dockerfile:1

FROM rust:1-bookworm AS builder

WORKDIR /src
COPY . .

RUN cargo build --release --locked -p fern_server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1000 fern \
    && useradd --system --uid 1000 --gid fern --home-dir /data --shell /usr/sbin/nologin fern \
    && mkdir -p /data \
    && chown -R fern:fern /data

COPY --from=builder /src/target/release/fern-server /usr/local/bin/fern-server

USER fern
WORKDIR /data

VOLUME /data

ENV FERN_LISTEN=0.0.0.0:8650 \
    FERN_DATA=/data/fern-server.db \
    FERN_GC_GRACE_HOURS=24

EXPOSE 8650

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD ["curl", "-fsS", "http://127.0.0.1:8650/health"]

ENTRYPOINT ["/usr/local/bin/fern-server"]
