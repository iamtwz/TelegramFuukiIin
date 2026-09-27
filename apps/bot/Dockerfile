# Official multi-platform images, pinned by digest. Keep Rust aligned with rust-toolchain.toml.
FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY apps/bot/ apps/bot/
COPY packages/verification-protocol/ packages/verification-protocol/
RUN cargo build --release --locked -p fuuki-iin-bot

FROM debian:bookworm-slim@sha256:3783cc01769c7b2b1b83a5c5ad96c815348e28ed7da68e2e3687004faa906251 AS runtime

LABEL org.opencontainers.image.title="TelegramFuukiIin" \
      org.opencontainers.image.description="Local Telegram moderation and join verification bot" \
      org.opencontainers.image.licenses="MIT"

# rustls uses the system trust store; SQLite is bundled into the binary.
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=build /build/target/release/fuuki-iin-bot /usr/local/bin/fuuki-iin-bot
COPY LICENSE /usr/share/doc/fuuki-iin/LICENSE
RUN groupadd --gid 10001 fuuki-iin \
    && useradd --uid 10001 --gid fuuki-iin --no-create-home --shell /usr/sbin/nologin fuuki-iin \
    && install -d -m 0700 -o fuuki-iin -g fuuki-iin /var/lib/fuuki-iin

WORKDIR /app
ENV DATABASE_PATH=/var/lib/fuuki-iin/fuuki-iin.sqlite
USER 10001:10001
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/fuuki-iin-bot"]
CMD ["run"]
