# CLAUDE.md — Personal site

## Overview

Hybrid personal site: server-rendered public pages (MiniJinja) + Vue 3 admin SPA embedded into the binary via `rust-embed`. PostgreSQL via SeaORM. Exposes a JSON API, OAuth2 (PKCE, RFC 7591), an MCP server for Claude, and an in-house AI assistant.

## Tech Stack

- **Backend:** Rust (edition 2024), Axum 0.8, Tokio
- **Database:** PostgreSQL via SeaORM 1.x; migrations run automatically on startup
- **Public rendering:** MiniJinja templates resolved via `DesignStore` (`src/design/`, `src/templates/`): `DESIGN_DIR` (dev-only) → the published design (`design/…` keys) → baked. Admins edit one shared draft (`design-draft/…`, `/api/design/draft/*`, every mutation broadcasts `design.draft_changed`) and publish it atomically (validate → snapshot to `design-history/{id}/` → mirror to `design/` → reload, rolled back on failure; a publish left pending is completed by the next reload/publish/start); every published version is kept and restorable into the draft. `design-draft.json` records the draft's base, so a publish over `design/` changed elsewhere answers 409 unless `?force=true`. `site_cli design push` writes into the draft; bucket edits + admin Reload still work. After the first publish `design/` is a full copy, so newer baked files are shadowed until reverted per file. Release builds compile every template at startup and recompile on a design reload; debug builds live-reload (rebuilt per render). See [`docs/architecture.md`](../docs/architecture.md#design-overrides) (#110, #115). Every template gets a typed context struct (`src/templates/context.rs`) — the generated [design contract](../docs/design-contract.md) (`make contract`; snapshot-tested) documents them; `templates::smoke::smoke_render` renders a design strictly against real data and lists template errors
- **Admin UI:** Vue 3 SPA (Pinia, Vue Router, Tailwind 4, Vite, TypeScript) — built into `client/dist/`, embedded via `rust-embed`, served at `/admin/*` with SPA fallback to `index.html`
- **Markdown:** pulldown-cmark with HTML-tag directives (`<page>`, `<image>`, `<file>`, `<gallery>`, `<fen>`, `<pgn>`, `<mermaid>`, `<json>`); `<fen>`/`<pgn>`/`<mermaid>`/`<json>` also accept an inline body, e.g. `<pgn>…</pgn>`. The full directive set is one source of truth: `MARKDOWN_EXTENSIONS_DOC` (`src/markdown/mod.rs`), shared verbatim by the MCP server instructions and the AI system prompt
- **Auth:** Argon2 password hashing, session cookies (`site_session`, 24 h), legacy service tokens, OAuth2 (PKCE)
- **MCP:** hand-rolled JSON-RPC 2.0 server at `POST /mcp` (the per-user MCP *client* the AI assistant consumes goes through `entanglement_runtime::mcp::HttpClient`)
- **AI:** `src/ai/` adapts a single process-wide `entanglement-core`/`-runtime`/`-provider` engine (`Holly`) into `AppState` — per-user sessions, DB-backed tool permissions, per-user MCP client, event-sourced history, sub-agent profiles, streamed over the WS hub
- **Export:** `src/export/` renders pages to PDF/reveal.js-slides on a **remote `mdcast-server`** via the thin `mdcast-client` 0.4 crate — the site compiles no typst and spawns no pandoc. `markdown::render_for_export` (#66) bridges every markdown directive (`<page>`/`<file>`/`<image>`/`<gallery>`/`<fen>`/`<pgn>`/`<mermaid>`/`<json>`) to plain markdown — real image refs/pipe tables/spliced page markdown, with synthesized fen/pgn/mermaid diagrams (`chess-diagram`/`mermaid-svg`) as in-memory assets — and `export::build_bundle` declares every referenced asset by sha256 (design templates from `design/mdcast/` shadow the server catalog; page images are digest-only from `files.hash`, bytes fetched from storage only on a server cache miss). `mdcast_api::BrandSpec` is sourced from `design/mdcast/brand.toml` (mapped from `design/assets/css/style.css`'s `:root` tokens, with brand-aware typst layouts `design/mdcast/typst/layouts/pdf/*.typ` and a reveal.js escape hatch `design/mdcast/revealjs/brand.css` alongside — all `DESIGN_DIR`-overridable) and travels as `request.brand`; splitting/classification happen server-side. `MDCAST_URL` unset → export routes answer 503; see [`docs/architecture.md`](../docs/architecture.md#export-mdcast) (#64–#68, mdcast 0.4 adoption)
- **Storage:** `src/storage/` — content-addressed blob store for file/thumbnail bytes over `db` (`file_blobs.data`, default), `fs` or `s3` (`object_store`; Garage in prod), picked by `STORAGE_KIND`, plus keyed objects (the design: `design/…`, `design-draft/…`, `design-history/…`) on every backend (`storage_objects` rows on `db`). Outage → 503. `site_cli storage migrate` copies blobs and keyed objects between backends; see [`docs/architecture.md`](../docs/architecture.md#storage) (#109, #114)
- **Logging:** tracing + tracing-subscriber with env filter

## Architecture (overview)

Three binaries (`site_server`, `site_migration`, `site_cli`) over one crate. `site_server` serves: server-rendered public pages (`/{*path}` catch-all → menu → page) via the `DesignStore`/`Templates` engine, the embedded Vue admin SPA at `/admin/*`, a session-cookie JSON API at `/api/*` (including a global WebSocket hub at `GET /api/ws`), an OAuth2 server, and a hand-rolled JSON-RPC MCP endpoint at `POST /mcp`. Content (pages, files, galleries) lives in PostgreSQL via SeaORM; page edits keep `diffy` revisions; files are content-addressed (SHA-256) with deduped `file_blobs`. The `src/ai/` subsystem wires a single `entanglement`-based engine into `AppState`, running a per-user agentic assistant over configurable LLM providers and per-user MCP servers, with turns streamed live over the WebSocket hub.

Two embed seams: `client/dist` (the SPA — generated, must be built before the binary) and `design/` (the baked default design bundle, overridden per deployment by `design/…` objects in storage — see [`docs/architecture.md`](../docs/architecture.md#design-overrides) — and locally by `DESIGN_DIR`).

> **Full reference — read before touching these areas:** the project structure, **data model**, **routes**, the **MCP tool list**, the **AI assistant** layout, and Docker all live in [`docs/architecture.md`](../docs/architecture.md).

## Build & Run

**Toolchain:** pinned in `rust-toolchain.toml` (`channel = "1.99.0"`, clippy + rustfmt) — the single source of truth rustup honors locally and in every CI workflow (`rustup toolchain install`, no `dtolnay/rust-toolchain`), so a local `make lint` predicts CI. To bump: edit `channel` and the Dockerfile `FROM rust:…` tag in the same change, then `make verify` and fix new fmt/clippy findings.

All build/test/dev flows go through the **`Makefile`**. The Vue admin SPA is
embedded into `site_server` via rust-embed (`#[folder = "client/dist"]`), so
**`client/dist` must exist before `cargo build`** — the targets enforce that.

```bash
make run        # build client/dist + run site_server on :3000 (needs DATABASE_URL)
make build      # build client + the binaries
make dev        # hot-reload admin SPA (vite)
make verify     # pre-"done" gate: lint + tests
make check      # fast cargo check
make contract   # regenerate docs/design-contract.md after changing a template context
make            # list all targets
```

Migrations & users (`make migrate` wraps the plain apply; the subcommands and
`site_cli` are run directly):

```bash
cargo run --bin site_migration              # apply all
cargo run --bin site_migration -- down      # rollback last
cargo run --bin site_migration -- fresh     # reset & reapply
cargo run --bin site_migration -- status
cargo run --bin site_cli -- create-user <username> <password>
cargo run --bin site_cli -- change-password <username> <password>
cargo run --bin site_cli -- storage migrate --from db   # or --from-dir <path>; into STORAGE_KIND
cargo run --bin site_cli -- design push <dir>          # upload a design folder into the shared design draft
cargo run --bin site_cli -- design contract            # print the template contract (make contract)
```

`/check` wraps `make verify`; `/site-mcp` exercises the MCP endpoint (local vs production).

Tests: `make test` (backend unit + integration + client), `make test-unit`, `make test-client`. See [`docs/testing.md`](../docs/testing.md) for how tests are organized and how to add them (backend `#[cfg(test)]`, client vitest specs, `tests/` integration harness).

## Environment

| Variable | Default | Purpose |
|---|---|---|
| `DATABASE_URL` | (required) | PostgreSQL connection string |
| `RUST_LOG` | `site=debug,tower_http=debug,info` | Tracing filter |
| `PORT` | `3000` | HTTP listen port |
| `DESIGN_DIR` | (unset) | Dev-only override folder (`templates/`, `assets/`, `mdcast/`), checked before storage overrides and the baked bundle. Debug builds read it live on each request; release builds freeze it into RAM at startup. Deployments keep their design in storage instead |
| `SERPER_API_KEY` | (unset) | Enables AI assistant `web_search` tool |
| `PUBLIC_URL` | (unset) | Public base URL used to build absolute `<loc>` entries in `/sitemap.xml` |
| `SELF_URL` | (unset) | Fallback base URL for the sitemap when `PUBLIC_URL` is unset |
| `STORAGE_KIND` | `db` | Blob backend for file/thumbnail bytes: `db` (`file_blobs.data`), `fs` or `s3`. Invalid config refuses the start |
| `STORAGE_DIR` | `./data` | Root for `STORAGE_KIND=fs` |
| `S3_ENDPOINT` / `S3_BUCKET` / `S3_REGION` / `S3_ACCESS_KEY_ID` / `S3_SECRET_ACCESS_KEY` / `S3_PATH_STYLE` | (unset) / `us-east-1` / `false` | `STORAGE_KIND=s3` settings (Garage: `https://s3.mmik.cz`, region `garage`, path-style) |
| `MDCAST_URL` | (unset) | Base URL of the remote `mdcast-server` rendering PDF/slides exports. Unset → export routes answer 503; nothing else degrades |
| `MDCAST_TOKEN` | (unset) | Bearer token for `mdcast-server`. Unset sends the literal placeholder `unauthenticated`, fine for a tokenless server |

## Conventions

- Migrations auto-run on server startup
- API protected by session-cookie middleware (`require_login_api`)
- MCP/OAuth protected by Bearer token middleware in handlers
- Page revisions store diffs (`diffy`), not full snapshots
- Files are content-addressed by SHA-256; `file_blobs` deduplicate. Bytes go through `Storage` (`src/storage/`, `state.storage`) — never read/write `file_blobs.data` or `storage_objects` directly; both are only filled by the `db` backend
- Service tokens have no expiry; OAuth access tokens last 1 h
- Always run `cargo check` after Rust changes; run the Vue build before serving the SPA (use `make`)
- **Keep [`docs/architecture.md`](../docs/architecture.md) current** — when a change adds/removes/renames a module, route, entity, env var, or MCP tool, update the matching section there in the same change
