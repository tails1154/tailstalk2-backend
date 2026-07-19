FROM alpine:3.21
RUN apk add --no-cache ca-certificates tzdata
COPY /home/tails1154/stoat/backend/deploy/revolt-delta /usr/local/bin/revolt-delta
EXPOSE 8000
ENTRYPOINT ["/usr/local/bin/revolt-delta"]
