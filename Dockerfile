# -- Frontend build --
FROM node:25.1.0-alpine AS frontend
WORKDIR /app/client
COPY client/package.json client/package-lock.json ./
RUN npm ci
COPY client/ ./
RUN npm run build

# -- Backend build --
# Keep the tag equal to `channel` in rust-toolchain.toml; copying that file in
# makes rustup enforce the pin even if the two drift.
FROM rust:1.99.0-bookworm AS backend
WORKDIR /app
COPY rust-toolchain.toml Cargo.toml Cargo.lock ./
COPY src/ src/
COPY design/ design/
COPY --from=frontend /app/client/dist client/dist
RUN cargo build --release --bin site_server --bin site_cli --bin site_migration

# -- Runtime --
FROM debian:bookworm-slim
# No pandoc/typst here: since mdcast 0.4 PDF/slides export happens on a
# separate `mdcast-server` deployment reached via MDCAST_URL. ca-certificates
# stays for TLS (to that server, among others).
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /app/target/release/site_server ./
COPY --from=backend /app/target/release/site_cli ./
COPY --from=backend /app/target/release/site_migration ./
EXPOSE 3000
ENTRYPOINT ["./site_server"]
