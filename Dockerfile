FROM rust:1.75-slim AS builder
WORKDIR /build
COPY . .
RUN cargo build --release -p arc-gateway

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/arc-gateway /usr/local/bin/arc-gateway
EXPOSE 8080 8443 9090
ENTRYPOINT ["arc-gateway"]
CMD ["--config", "/etc/arc/arc.toml"]
