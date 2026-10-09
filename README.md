# Personal Site

Rust/Axum personal site. Server-rendered public pages plus a Vue 3 admin SPA. Includes a JSON API, OAuth2 (PKCE, RFC 7591), an MCP server for Claude integration, and a built-in AI assistant subsystem.

## Requirements

- Rust (edition 2024)
- Node.js (for building the Vue admin client)
- PostgreSQL
- Docker & Docker Compose (for containerized setup)

## Quick Start with Docker Compose

`docker-compose.yml` defines two services: `db` (Postgres 17, exposed on host port `5434`) and `app` (multi-stage build: Node → Rust → debian-slim). The `app` service is gated behind the `full` profile.

```bash
# Database only
docker compose up -d db

# Database + app
docker compose --profile full up -d --build
```

The app is available at `http://localhost:3000`.

## Local development

```bash
# 1) Build the Vue admin client (embedded into the binary via rust-embed)
cd client
npm ci
npm run build
cd ..

# 2) Run the server (requires DATABASE_URL in .env or environment)
cargo run --bin site_server

# Type-check without running
cargo check
```

For frontend iteration, run `npm run dev` in `client/` against the running server.

## Creating a User

```bash
# Without Docker
cargo run --bin site_cli -- create-user <username> <password>
cargo run --bin site_cli -- change-password <username> <password>

# With Docker Compose
docker compose exec app ./site_cli create-user <username> <password>
```

## Database Migrations

Migrations run automatically on server startup. To manage them manually:

```bash
cargo run --bin site_migration              # apply all pending
cargo run --bin site_migration -- down      # rollback last
cargo run --bin site_migration -- fresh     # reset & reapply all
cargo run --bin site_migration -- status    # show status

# With Docker Compose
docker compose exec app ./site_migration
```

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `DATABASE_URL` | PostgreSQL connection string | required (compose uses `postgres://blog:blog@db:5432/blog`) |
| `RUST_LOG` | Log level filter | `site=debug,tower_http=debug,info` |
| `PORT` | HTTP listen port | `3000` |
| `DESIGN_DIR` | Override folder for `{templates, assets/{css,js,img}}`, checked before the baked `design/` bundle (debug: live reload; release: frozen into RAM at startup) | unset |
| `SERPER_API_KEY` | Optional — enables the `web_search` tool inside the AI assistant | unset |
| `MDCAST_URL` | Base URL of the remote [`mdcast-server`](https://github.com/xmiksay/mdcast) that renders PDF/slides exports — export routes answer 503 while unset | unset |
| `MDCAST_TOKEN` | Bearer token for `mdcast-server` (unset sends a placeholder, fine for a tokenless server) | unset |
| `STORAGE_KIND` | Where file and thumbnail bytes live: `db` (`file_blobs.data`), `fs` or `s3`. Move existing blobs with `site_cli storage migrate` before switching | `db` |
| `STORAGE_DIR` | Root directory for `STORAGE_KIND=fs` (created at start) | `./data` |
| `S3_ENDPOINT` | S3-compatible endpoint for `STORAGE_KIND=s3` (empty → AWS) | unset |
| `S3_BUCKET` | Bucket (required for `s3`) | unset |
| `S3_REGION` | Region | `us-east-1` |
| `S3_ACCESS_KEY_ID` / `S3_SECRET_ACCESS_KEY` | Credentials (required for `s3`) | unset |
| `S3_PATH_STYLE` | `true` for Garage/MinIO (`{endpoint}/{bucket}/{key}`); `false` puts the bucket into the host | `false` |
| `PUBLIC_URL` | Public base URL for absolute `<loc>` entries in `/sitemap.xml` | unset (falls back to `SELF_URL`, then `http://localhost:3000`) |
| `SELF_URL` | Fallback base URL for the sitemap when `PUBLIC_URL` is unset | unset |

## Testing

```bash
make test          # backend unit + integration + client
make test-unit     # Rust: cargo test --lib --bins
make test-client   # Vue admin SPA: vitest
make verify        # lint + all tests (pre-"done" gate)
```

See [`docs/testing.md`](docs/testing.md) for how tests are organized and how to
add backend unit tests, client (vitest) specs, and `tests/` integration tests.

## Docker image (standalone)

```bash
docker build -t site .
docker run -e DATABASE_URL=postgres://... -p 3000:3000 site
```

The Dockerfile runs the Node stage first (Vue build into `client/dist/`), then the Rust stage (`cargo build --release`), and ships `site_server`, `site_migration`, and `site_cli` in a slim runtime image.
