# CLAUDE.md — Personal site

## Overview

Hybrid personal site: server-rendered public pages (MiniJinja) + Vue 3 admin SPA embedded into the binary via `rust-embed`. PostgreSQL via SeaORM. Exposes a JSON API, OAuth2 (PKCE, RFC 7591), an MCP server for Claude, and an in-house AI assistant.

## Tech Stack

- **Backend:** Rust (edition 2024), Axum 0.8, Tokio
- **Database:** PostgreSQL via SeaORM 1.x; migrations run automatically on startup
- **Public rendering:** MiniJinja templates resolved via `AssetStore` (`src/templates.rs`). Release builds compile every template at startup; debug builds live-reload (rebuilt per render, so edits show up on the next request)
- **Admin UI:** Vue 3 SPA (Pinia, Vue Router, Tailwind 4, Vite, TypeScript) — built into `client/dist/`, embedded via `rust-embed`, served at `/admin/*` with SPA fallback to `index.html`
- **Markdown:** pulldown-cmark with HTML-tag directives (`<page>`, `<image>`, `<file>`, `<gallery>`, `<fen>`, `<pgn>`); `<fen>`/`<pgn>` also accept an inline body, e.g. `<pgn>…</pgn>`
- **Auth:** Argon2 password hashing, session cookies (`site_session`, 24 h), legacy service tokens, OAuth2 (PKCE)
- **MCP:** `rmcp` crate; server at `POST /mcp`
- **AI:** local subsystem in `src/ai/` (LLM providers, tool registry, MCP client, tool permissions, agentic loop)
- **Logging:** tracing + tracing-subscriber with env filter

## Project Structure

```
src/
  bin/
    site_server.rs        # HTTP server, port 3000
    site_migration.rs     # Migration CLI (up/down/fresh/status)
    site_cli.rs           # create-user, change-password
  routes/
    public/               # catch-all, files, search, sitemap, tags
    api/                  # auth, pages, tags, files, galleries, menu,
                          # tokens, markdown, paths, assistant, llm,
                          # tool-permissions
    mcp.rs                # MCP JSON-RPC endpoint
    oauth.rs              # OAuth2 server (register/authorize/token/well-known)
    revision.rs
  entity/                 # SeaORM entity models
    user, token, page, page_revision, tag, menu,
    file, file_blob, file_thumbnail, gallery,
    oauth_{client,code,token},
    llm_{provider,model},
    assistant_{session,message},
    user_mcp_server, tool_permission
  migration/              # m_001 … m_022
  ai/                     # config, handlers, llm, local_tools,
                          # loop_driver, mcp_client, tool_permissions,
                          # tool_registry
  auth.rs assets.rs config.rs files.rs
  markdown.rs path_util.rs repo state.rs templates.rs

client/                   # Vue 3 SPA
  src/  dist/             # dist/ is embedded into the binary

assets/common/            # Baked static bundle (via rust-embed); fallback layer
  css/  js/  img/  templates/
assets/<other>/           # Not baked — example override bundles you can point
                          # ASSETS_DIR at (e.g. assets/miksanik.net)
```

Asset/template resolution (see `src/assets.rs`, `AssetStore`):
`ASSETS_DIR` override folder → baked `assets/common/` → not found.
The override folder mirrors the bundle layout (`templates/`, `css/`, `js/`, `img/`)
and lets a deployment ship its own assets as a plain folder instead of
recompiling. With no `ASSETS_DIR` set, only the baked `common` bundle is used.

Templates (`src/templates.rs`, `Templates`) sit on top of the same `AssetStore`:
release builds compile every template once at startup (frozen, shared); debug
builds rebuild the environment from the assets on each render (live reload).

## Build & Run

```bash
# 1) Build the Vue client (embedded into the server binary)
(cd client && npm ci && npm run build)

# 2) Run the server (requires DATABASE_URL)
cargo run --bin site_server

# Type-check
cargo check

# Migrations
cargo run --bin site_migration              # apply all
cargo run --bin site_migration -- down      # rollback last
cargo run --bin site_migration -- fresh     # reset & reapply
cargo run --bin site_migration -- status

# Users
cargo run --bin site_cli -- create-user <username> <password>
cargo run --bin site_cli -- change-password <username> <password>
```

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `DATABASE_URL` | (required) | PostgreSQL connection string |
| `RUST_LOG` | `site=debug,tower_http=debug,info` | Tracing filter |
| `PORT` | `3000` | HTTP listen port |
| `ASSETS_DIR` | (unset) | Override folder for `{templates,css,js,img}`, checked before the baked `common` bundle. Debug builds read it live on each request; release builds freeze it into RAM at startup |
| `SERPER_API_KEY` | (unset) | Enables AI assistant `web_search` tool |

## Data Model

```
users               id, username unique, password_hash (Argon2)
tokens              id, nonce unique, user_id, expires_at?, label?, is_service
                    -- is_service=false → 24h session; is_service=true → service token

pages               id, path unique, summary?, markdown, tag_ids INT[],
                    private, audit fields. Fulltext index (m_022).
page_revisions      id, page_id, seq, prev_markdown, diff (diffy), audit
tags                id, name unique, description?
menus               id, path unique, markdown, private (m_008)

files               id, path unique (m_017), hash (SHA-256), mimetype,
                    size_bytes, description?, audit
file_blobs          hash PK, data bytea, size_bytes (deduped by hash)
file_thumbnails     file_id PK, hash, width, height, mimetype
galleries           id, path unique (m_020), title, description?,
                    file_ids INT[], audit

oauth_clients       id, client_id unique, client_secret?, client_name,
                    redirect_uris JSON
oauth_codes         id, code unique, client_id, user_id, redirect_uri,
                    code_challenge (PKCE), expires_at, used
oauth_tokens        id, access_token unique, refresh_token unique,
                    client_id, user_id, expires_at, revoked

llm_providers       id, label, kind (anthropic|ollama|gemini), api_key?, base_url?
llm_models          id, provider_id, label, model wire-id, is_default
assistant_sessions  id, user_id, title, provider/model snapshots, model_id?,
                    enabled_mcp_server_ids JSONB (m_018), timestamps
assistant_messages  id, session_id, seq, role, content JSON
user_mcp_servers    id, user_id, name, url, enabled, forward_user_token,
                    headers JSON
tool_permissions    id, user_id, name pattern, effect (allow|deny|prompt),
                    priority
```

## Routes

### Public (server-rendered)

| Path | Method | Description |
|---|---|---|
| `/files/{hash}` | GET | Full file (content-addressed, cacheable) |
| `/files/{hash}/nahled` | GET | Thumbnail |
| `/tag/{name}` | GET | Tag listing |
| `/search?q=...` | GET | Fulltext search |
| `/sitemap.xml` | GET | Sitemap |
| `/static/{*path}` | GET | Static assets (`ASSETS_DIR` override → baked `assets/common/{css,js,img}`) |
| `/{*path}` | GET | Catch-all: menu → page → 404 |

### Admin SPA

| Path | Method | Description |
|---|---|---|
| `/admin` | GET | SPA entry (`index.html`) |
| `/admin/{*path}` | GET | Static from `client/dist/` via `rust-embed`; SPA fallback to `index.html` |

### JSON API `/api/*` (session cookie required)

`auth/{login,logout,me}`, `pages` CRUD + `paths` + revision restore, `tags` CRUD, `files` CRUD (multipart upload, 50 MB), `galleries` CRUD + `paths`, `menu` CRUD, `tokens` (list/create/delete), `markdown/render`, `paths/children`, `assistant/*`, `llm/{providers,models}` CRUD, `tool-permissions` CRUD.

### OAuth2 + MCP

| Path | Method | Description |
|---|---|---|
| `POST /mcp` | POST | MCP JSON-RPC 2.0 (Bearer auth — service token or OAuth access token) |
| `POST /oauth/register` | POST | Dynamic client registration (RFC 7591) |
| `GET/POST /oauth/authorize` | GET, POST | PKCE authorization code (10 min) |
| `POST /oauth/token` | POST | `authorization_code` (PKCE verify) or `refresh_token` |
| `GET /.well-known/oauth-authorization-server` | GET | Metadata |
| `GET /.well-known/oauth-protected-resource` | GET | Marks `/mcp` as the protected resource |

## MCP Server

`POST /mcp` exposes JSON-RPC 2.0 with these tools (defined in `src/routes/mcp.rs`):

- **Pages:** `read_page`, `edit_page`, `search_pages` (prefix/tag/q + limit/offset), `delete_page`
- **Tags:** `list_tags`, `read_tag`, `create_tag`, `update_tag`, `delete_tag`
- **Files:** `list_files`, `create_file`, `read_file`, `update_file`, `delete_file`
- **Galleries:** `list_galleries`, `read_gallery`, `create_gallery`, `update_gallery`, `delete_gallery`

`SERVER_INSTRUCTIONS` are loaded from a private `CLAUDE` page if present (editable via admin UI), else fall back to a default constant in `src/routes/mcp.rs`. Tool/parameter descriptions live in `handle_tools_list()`.

Auth: `Authorization: Bearer <token>` — accepts both service tokens (legacy, no expiry) and OAuth2 access tokens (1 h, refreshable). Handler resolves to `user_id` for audit fields.

## AI Assistant (`src/ai/`)

- `loop_driver.rs` — agentic loop, streams responses
- `tool_registry.rs` — unified registry of local tools and active MCP servers
- `mcp_client/` — connects to user-configured MCP servers (with optional `forward_user_token`)
- `tool_permissions.rs` — evaluates allow/deny/prompt rules ordered by priority
- `local_tools/` — built-in tools (web search via Serper)
- `llm/` — adapters for `anthropic`, `ollama`, `gemini`

Configured per user via the admin SPA: `/admin/{providers,models,assistant,mcp-servers,tool-permissions}`. Provider API keys live in `llm_providers.api_key` (set through the UI, never in `.env`).

## Docker

```bash
docker build -t site .
docker run -e DATABASE_URL=... -p 3000:3000 site
```

`docker-compose.yml`: `db` (Postgres 17-alpine, host port 5434) + `app` (gated behind `profiles: ["full"]`).

## Conventions

- Migrations auto-run on server startup
- API protected by session-cookie middleware (`require_login_api`)
- MCP/OAuth protected by Bearer token middleware in handlers
- Page revisions store diffs (`diffy`), not full snapshots
- Files are content-addressed by SHA-256; `file_blobs` deduplicate
- Service tokens have no expiry; OAuth access tokens last 1 h
- Always run `cargo check` after Rust changes; run the Vue build before serving the SPA
