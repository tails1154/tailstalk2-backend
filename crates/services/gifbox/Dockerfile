# Build Stage
FROM ghcr.io/stoatchat/base:latest AS builder

# Bundle Stage
FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home app
COPY --from=builder /home/rust/src/target/release/revolt-gifbox ./

EXPOSE 14706
USER app
CMD ["./revolt-gifbox"]
