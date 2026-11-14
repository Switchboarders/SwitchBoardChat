########################################################################################################################
# iowa build stage
########################################################################################################################
# Built for the machine's own architecture (x86_64 servers, arm64 such as a Raspberry Pi 4/5 with a 64-bit OS)
FROM rust:1.96.1-slim-bookworm AS switchboard-build
RUN apt update && \
    apt install -y musl-tools musl-dev libssl-dev pkg-config libsqlite3-dev adduser && \
    update-ca-certificates && \
    rm -rf /var/lib/apt/lists/*
RUN adduser \
    --disabled-password \
    --gecos "" \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --uid 10001 \
    "sb-chat"
# Copy only dependency files first - this layer will be cached unless dependencies change
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml

# Create a dummy main.rs to build dependencies
RUN mkdir -p src && \
    echo "fn main() {}" > src/main.rs && \
    cargo build --release && \
    rm -rf src

# Now copy the actual source code
# This invalidates the cache only when source changes, but dependencies are already built
COPY ./src ./src

# Touch src/main.rs to force rebuild of only your code (not dependencies)
RUN touch src/main.rs && \
    cargo build --release
# Reactiver pour release --release
########################################################################################################################
# switchboard-chat image
########################################################################################################################
#FROM scratch as final
FROM debian:bookworm-slim AS final
ENV APP_DATA_DIR=/app/data
ENV RUST_BACKTRACE=1
COPY --from=switchboard-build /etc/passwd /etc/passwd
COPY --from=switchboard-build /etc/group /etc/group
RUN apt-get update && \
    apt-get install -y --no-install-recommends libsqlite3-0 cron ca-certificates openssl util-linux && \
    rm -rf /var/lib/apt/lists/*
RUN mkdir -p /app/data/logs /app/data/db /app/data/web_server_keys && \
    chown -R sb-chat:sb-chat /app
COPY --chmod=755 --chown=sb-chat:sb-chat create_ssl_certificates.sh /app/ 
# .env is NOT copied into the image (it holds secrets): docker-compose passes it with env_file
COPY --chmod=755 --chown=sb-chat:sb-chat entrypoint.sh /app/ 
COPY --chmod=755 --chown=sb-chat:sb-chat ./src/nameslist/names.csv /app/  

COPY --from=switchboard-build --chown=sb-chat:sb-chat ./target/release/switchboard-chat /app/switchboard-chat
# Cron job for automatic certificates renew
COPY --chmod=0644 container_cronjob /etc/crontab


# entrypoint.sh starts as root only to fix the data volume's ownership and start the certificate
# renewal (cron), then runs the bridge as sb-chat (setpriv): the bridge itself never runs as root.
ENTRYPOINT ["/app/entrypoint.sh"]
CMD ["/app/switchboard-chat"]
