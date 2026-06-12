# ---- build stage ------------------------------------------------------
FROM rust:1.96-slim-bookworm AS build
WORKDIR /src

# Build with the committed sqlx offline artifact; no database needed.
ENV SQLX_OFFLINE=true

COPY . .
RUN cargo build --release --bin leaf --bin leaf-migrate

# ---- runtime stage -----------------------------------------------------
FROM debian:bookworm-slim AS runtime

# ffmpeg: video poster-frame extraction (media pipeline, Phase 5+).
# ca-certificates: TLS to Discord and R2.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg \
    && rm -rf /var/lib/apt/lists/*

RUN groupadd --system leaf && useradd --system --gid leaf --create-home leaf

COPY --from=build /src/target/release/leaf /usr/local/bin/leaf
COPY --from=build /src/target/release/leaf-migrate /usr/local/bin/leaf-migrate

USER leaf
ENV DATA_DIR=/data
VOLUME /data
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/leaf"]
