FROM debian:sid-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libssl3 \
    && rm -rf /var/lib/apt/lists/*

COPY revolt-voice-ingress /usr/local/bin/revolt-voice-ingress
RUN chmod +x /usr/local/bin/revolt-voice-ingress

ENTRYPOINT ["/usr/local/bin/revolt-voice-ingress"]
