# ---- frontend build ----------------------------------------------------
FROM node:22-bookworm-slim AS frontend
WORKDIR /app
# Install against the committed lockfile first for layer caching.
COPY activity/package.json activity/package-lock.json ./
RUN npm ci
COPY activity/ ./
RUN npm run build

# ---- rust build --------------------------------------------------------
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

# /data must exist owned by the runtime user BEFORE the VOLUME declaration:
# named volumes copy ownership from the image path at first mount. Without
# this, the mountpoint is root-owned and the non-root process cannot write
# leaf.conf or the database (the classic walpurgisbot-v2 EACCES).
RUN mkdir -p /data && chown leaf:leaf /data

COPY --from=build /src/target/release/leaf /usr/local/bin/leaf
COPY --from=build /src/target/release/leaf-migrate /usr/local/bin/leaf-migrate
# The built gallery; leaf-server serves it from STATIC_DIR.
COPY --from=frontend /app/dist /app/dist

USER leaf
ENV DATA_DIR=/data
ENV STATIC_DIR=/app/dist
VOLUME /data
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/leaf"]
