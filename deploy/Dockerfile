FROM debian:sid-slim
RUN apt-get update && apt-get install -y libssl3 && rm -rf /var/lib/apt/lists/*
COPY revolt-delta /usr/local/bin/revolt-delta
RUN chmod +x /usr/local/bin/revolt-delta
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/revolt-delta"]
