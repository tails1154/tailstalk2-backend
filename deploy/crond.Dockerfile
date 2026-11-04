FROM debian:sid-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY revolt-crond /revolt-crond
RUN chmod +x /revolt-crond
CMD ["/revolt-crond"]
