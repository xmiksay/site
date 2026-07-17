# CLAUDE.md — Personal site

## Overview

Hybrid personal site: server-rendered public pages (MiniJinja) + Vue 3 admin SPA embedded into the binary via `rust-embed`. PostgreSQL via SeaORM. Exposes a JSON API, OAuth2 (PKCE, RFC 7591), an MCP server for Claude, and an in-house AI assistant.

## Tech Stack

- **Backend:** Rust (edition 2024), Axum 0.8, Tokio
- **Database:** PostgreSQL via SeaORM 1.x; migrations run automatically on startup
- **Public rendering:** MiniJinja templates resolved via `DesignStore` (`src/design.rs`, `src/templates.rs`). Release builds compile every template at startup; debug builds live-reload (rebuilt per render, so edits show up on the next request)
- **Admin UI:** Vue 3 SPA (Pinia, Vue Router, Tailwind 4, Vite, TypeScript) — built into `client/dist/`, embedded via `rust-embed`, served at `/admin/*` with SPA fallback to `index.html`
- **Markdown:** pulldown-cmark with HTML-tag directives (`<page>`, `<image>`, `<file>`, `<gallery>`, `<fen>`, `<pgn>`, `<mermaid>`, `<json>`); `<fen>`/`<pgn>`/`<mermaid>`/`<json>` also accept an inline body, e.g. `<pgn>…</pgn>`. The full directive set is one source of truth: `MARKDOWN_EXTENSIONS_DOC` (`src/markdown.rs`), shared verbatim by the MCP server instructions and the AI system prompt
- **Auth:** Argon2 password hashing, session cookies (`site_session`, 24 h), legacy service tokens, OAuth2 (PKCE)
- **MCP:** hand-rolled JSON-RPC 2.0 server at `POST /mcp` (the per-user MCP *client* the AI assistant consumes goes through `entanglement_runtime::mcp::HttpClient`)
- **AI:** `src/ai/` adapts a single process-wide `entanglement-core`/`-runtime`/`-provider` engine (`Holly`) into `AppState` — per-user sessions, DB-backed tool permissions, per-user MCP client, event-sourced history, sub-agent profiles, streamed over the WS hub
- **Logging:** tracing + tracing-subscriber with env filter

## Architecture (overview)

Three binaries (`site_server`, `site_migration`, `site_cli`) over one crate. `site_server` serves: server-rendered public pages (`/{*path}` catch-all → menu → page) via the `DesignStore`/`Templates` engine, the embedded Vue admin SPA at `/admin/*`, a session-cookie JSON API at `/api/*` (including a global WebSocket hub at `GET /api/ws`), an OAuth2 server, and a hand-rolled JSON-RPC MCP endpoint at `POST /mcp`. Content (pages, files, galleries) lives in PostgreSQL via SeaORM; page edits keep `diffy` revisions; files are content-addressed (SHA-256) with deduped `file_blobs`. The `src/ai/` subsystem wires a single `entanglement`-based engine into `AppState`, running a per-user agentic assistant over configurable LLM providers and per-user MCP servers, with turns streamed live over the WebSocket hub.

Two embed seams: `client/dist` (the SPA — generated, must be built before the binary) and `design/` (the baked default design bundle, overridable at runtime via `DESIGN_DIR`).

> **Full reference — read before touching these areas:** the project structure, **data model**, **routes**, the **MCP tool list**, the **AI assistant** layout, and Docker all live in [`docs/architecture.md`](../docs/architecture.md).

## Build & Run

All build/test/dev flows go through the **`Makefile`**. The Vue admin SPA is
embedded into `site_server` via rust-embed (`#[folder = "client/dist"]`), so
**`client/dist` must exist before `cargo build`** — the targets enforce that.

```bash
make run        # build client/dist + run site_server on :3000 (needs DATABASE_URL)
make build      # build client + the binaries
make dev        # hot-reload admin SPA (vite)
make verify     # pre-"done" gate: lint + tests
make check      # fast cargo check
make            # list all targets
```

Migrations & users (run as needed — not wrapped by make):

```bash
cargo run --bin site_migration              # apply all
cargo run --bin site_migration -- down      # rollback last
cargo run --bin site_migration -- fresh     # reset & reapply
cargo run --bin site_migration -- status
cargo run --bin site_cli -- create-user <username> <password>
cargo run --bin site_cli -- change-password <username> <password>
```

`/check` wraps `make verify`; `/site-mcp` exercises the MCP endpoint (local vs production).

Tests: `make test` (backend unit + integration + client), `make test-unit`, `make test-client`. See [`docs/testing.md`](../docs/testing.md) for how tests are organized and how to add them (backend `#[cfg(test)]`, client vitest specs, future integration harness).

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `DATABASE_URL` | (required) | PostgreSQL connection string |
| `RUST_LOG` | `site=debug,tower_http=debug,info` | Tracing filter |
| `PORT` | `3000` | HTTP listen port |
| `DESIGN_DIR` | (unset) | Override folder for `{templates, assets/{css,js,img}}`, checked before the baked `design/` bundle. Debug builds read it live on each request; release builds freeze it into RAM at startup |
| `SERPER_API_KEY` | (unset) | Enables AI assistant `web_search` tool |
| `PUBLIC_URL` | (unset) | Public base URL used to build absolute `<loc>` entries in `/sitemap.xml` |
| `SELF_URL` | (unset) | Fallback base URL for the sitemap when `PUBLIC_URL` is unset |

## Conventions

- Migrations auto-run on server startup
- API protected by session-cookie middleware (`require_login_api`)
- MCP/OAuth protected by Bearer token middleware in handlers
- Page revisions store diffs (`diffy`), not full snapshots
- Files are content-addressed by SHA-256; `file_blobs` deduplicate
- Service tokens have no expiry; OAuth access tokens last 1 h
- Always run `cargo check` after Rust changes; run the Vue build before serving the SPA (use `make`)
- **Keep [`docs/architecture.md`](../docs/architecture.md) current** — when a change adds/removes/renames a module, route, entity, env var, or MCP tool, update the matching section there in the same change
