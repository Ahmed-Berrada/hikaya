# syntax=docker/dockerfile:1
# Build context: the repository root (the hub depends on crates/hikaya-core).
#   docker build -f hub/Dockerfile -t hikaya-hub .

FROM rust:1.91-bookworm AS build
# perl and make build the vendored OpenSSL that SQLCipher links against.
RUN apt-get update && apt-get install -y --no-install-recommends perl make \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY crates/hikaya-core crates/hikaya-core
COPY hub hub
WORKDIR /src/hub
RUN cargo build --release --locked && strip target/release/hikaya-hub

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin hikaya
COPY --from=build /src/hub/target/release/hikaya-hub /usr/local/bin/hikaya-hub
USER hikaya
ENV HIKAYA_HUB_DATA=/data \
    HIKAYA_HUB_LISTEN=0.0.0.0:8787 \
    RUST_LOG=hikaya_hub=info
VOLUME ["/data"]
EXPOSE 8787
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD curl -fs http://127.0.0.1:8787/api/status >/dev/null || exit 1
ENTRYPOINT ["/usr/local/bin/hikaya-hub"]
