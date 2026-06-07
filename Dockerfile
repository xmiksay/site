# -- Frontend build --
FROM node:25.1.0-alpine AS frontend
WORKDIR /app/client
COPY client/package.json client/package-lock.json ./
RUN npm ci
COPY client/ ./
RUN npm run build

# -- Backend build --
FROM rust:1.88-bookworm AS backend
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY design/ design/
COPY --from=frontend /app/client/dist client/dist
RUN cargo build --release --bin site_server --bin site_cli --bin site_migration

# -- Runtime --
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /app/target/release/site_server ./
COPY --from=backend /app/target/release/site_cli ./
COPY --from=backend /app/target/release/site_migration ./
EXPOSE 3000
ENTRYPOINT ["./site_server"]
