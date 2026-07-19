FROM debian:bookworm-slim

COPY revolt-delta /usr/local/bin/revolt-delta
RUN chmod +x /usr/local/bin/revolt-delta
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/revolt-delta"]
