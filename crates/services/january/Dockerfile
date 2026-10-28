FROM ghcr.io/stoatchat/proxy:v0.13.6

# Keep the upstream January runtime libraries and replace only its binary.
COPY revolt-january /home/nonroot/revolt-january

EXPOSE 14705
USER nonroot
WORKDIR /home/nonroot
CMD ["./revolt-january"]
