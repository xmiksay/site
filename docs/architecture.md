# site — architecture

Deep reference for the personal site. The brief [`.claude/CLAUDE.md`](../.claude/CLAUDE.md) carries the overview, stack, build/run, environment, and conventions, and points here. **Keep this current:** when a change adds/removes/renames a module, route, entity, env var, or MCP tool, update the matching section here in the same change.

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
  auth.rs config.rs design.rs files.rs
  markdown.rs path_util.rs repo state.rs templates.rs

client/                   # Vue 3 SPA
  src/  dist/             # dist/ is embedded into the binary

design/                   # Baked default design bundle (via rust-embed)
  templates/              # rendered by the template engine
  assets/                 # served statically under /assets/* (css/ js/ img/)
```

Design/template resolution (see `src/design.rs`, `DesignStore`):
`DESIGN_DIR` override folder → baked `design/` → not found.
The override folder mirrors the bundle layout (`templates/`, `assets/{css,js,img}`)
and lets a deployment ship its own design as a plain folder instead of
recompiling. With no `DESIGN_DIR` set, only the baked `design/` bundle is used.

Templates (`src/templates.rs`, `Templates`) sit on top of the same `DesignStore`:
release builds compile every template once at startup (frozen, shared); debug
builds rebuild the environment from the assets on each render (live reload).

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
| `/assets/{*path}` | GET | Static files (`DESIGN_DIR` override → baked `design/assets/{css,js,img}`) |
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
