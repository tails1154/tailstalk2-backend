FROM debian:sid-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
COPY revolt-bonfire /usr/local/bin/revolt-bonfire
RUN chmod +x /usr/local/bin/revolt-bonfire
EXPOSE 14703
ENTRYPOINT ["/usr/local/bin/revolt-bonfire"]
