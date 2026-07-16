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
                          # tool-permissions, ws
    mcp.rs                # MCP JSON-RPC endpoint
    oauth.rs              # OAuth2 server (register/authorize/token/well-known)
    revision.rs
    ws.rs                 # global WebSocket hub (WsHub) + GET /api/ws upgrade
  entity/                 # SeaORM entity models
    user, token, page, page_revision, tag, menu,
    file, file_blob, file_thumbnail, gallery,
    oauth_{client,code,token},
    llm_{provider,model},
    assistant_{session,event},
    user_mcp_server, tool_permission
  migration/              # m_001 … m_023
  ai/                     # config, handlers, tool_permissions, ws_bridge —
                          # plus the entanglement-core/-runtime engine
                          # adapters: engine, catalog, mcp, persistence,
                          # policy, projection/, tools/. (llm, local_tools,
                          # mcp_client, tool_registry are the pre-engine-swap
                          # modules — unreachable from AppState, kept
                          # compiling only until issue #15 Phase 4 deletes
                          # them.)
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
                    enabled_mcp_server_ids JSONB (m_018),
                    engine_session_id? unique (m_023 — the engine's root
                    SessionId string, "u{user_id}:{uuid}"; nullable since
                    pre-engine-swap rows never get one back), timestamps
assistant_events    id, root_session_id (engine SessionId string, not a DB FK —
                    the engine has no notion of assistant_sessions.id),
                    payload JSONB (a serialized entanglement_runtime
                    LogRecord), created_at (m_023). Event-sourced log that
                    replaced the old per-message assistant_messages table;
                    `ai::projection::project` folds a session's rows into the
                    {role, content} shape the admin client renders.
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

`auth/{login,logout,me}`, `pages` CRUD + `paths` + revision restore, `tags` CRUD, `files` CRUD (multipart upload, 50 MB), `galleries` CRUD + `paths`, `menu` CRUD, `tokens` (list/create/delete), `markdown/render`, `paths/children`, and everything under `assistant/*`: `sessions` CRUD + `sessions/{id}/messages` + `sessions/{id}/messages/{message_id}/approve`, `mcp-servers` CRUD, `providers` CRUD, `models` CRUD, `permissions` CRUD (tool-permission rules).

| Path | Method | Description |
|---|---|---|
| `/api/ws` | GET (upgrade) | Global authenticated WebSocket — see below |

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

As of issue #15 Phase 1, the assistant runs on a single process-wide engine
(`entanglement-core`/`-runtime`/`-provider`) instead of the old bespoke
agentic loop — one `Holly` actor for every tenant, sessions namespaced
`u{user_id}:{uuid}` (`ai::engine::session_id_for_user`/`user_id_from_session`).
`AppState` exposes it as a single field, `agent_engine: Arc<SiteEngine>`
(`src/state.rs`); `SiteEngine::spawn` wires all of the pieces below at startup.

- `engine.rs` — `SiteEngine`: spawns `Holly`, the tool executor, the
  `assistant_events` persistence subscriber, the system-prompt refresh task,
  and a hibernate-watcher task that evicts a session from the in-process
  "live" cache when `Holly`'s own idle-TTL sweep (`EngineConfig.idle_ttl`,
  30 min) or a manual hibernate retires it — otherwise the next message to
  that session would skip `resume` and get a blank in-memory session despite
  intact history in `assistant_events`.
- `catalog.rs` — `SiteCatalog`: builds `entanglement_provider::LlmFactory`/
  `ModelResolver` closures from `llm_providers`/`llm_models`; `refresh()` is
  called after provider/model CRUD.
- `policy.rs` — `SitePolicy`: implements the engine's `PermissionResolver` +
  `GrantStore` over the existing `tool_permissions` table (same
  priority/wildcard rules as before, in `tool_permissions.rs`).
- `mcp.rs` — `SiteMcp`: per-user MCP tool discovery/routing over
  `entanglement_runtime::mcp::HttpClient`, replacing `mcp_client/`'s
  `UserMcpManager`; tools are named `"{server}__{tool}"`.
- `persistence.rs` — `DbSink`: the engine's `RecordSink`, appending every
  `LogRecord` to `assistant_events`; also the lazy session-resume and
  session-delete helpers.
- `projection/` — pure fold of a session's `assistant_events` rows into the
  `{role, content}` shape the admin client renders (`role` one of
  `user | assistant | tool_result | error`).
- `tools/` — the built-in (non-MCP) tool vocabulary (pages/tags/files/
  galleries CRUD + `web_search`/`web_fetch`), ported to
  `entanglement_runtime::tools::Tool`.
- `tool_permissions.rs` — the allow/deny/prompt rule evaluator `policy.rs`
  wraps (unchanged: first match wins, ordered by `priority ASC, id ASC`,
  trailing `*` is a prefix wildcard).
- `handlers/` — `/api/assistant/*`: `sessions/` (CRUD + `messages`/`approve`,
  which drive a turn through `Holly` and project `assistant_events` on the
  way out), `mcp_servers.rs`, `providers.rs`, `models.rs`, `permissions.rs`.
- `ws_bridge.rs` — a single process-wide task subscribing to
  `agent_engine.holly.subscribe()` (issue #16): forwards the engine's
  content/lifecycle `OutEvent`s (`Status`, `TextDelta`, `ReasoningDelta`,
  `ToolCallDelta`, `ToolCall`, `ToolRequest`, `ToolOutput`, `Done`, `Error`,
  `SessionHibernated`) to `WsHub` as `assistant.*` envelopes, resolving each
  event's `SessionId` to both the owning `user_id`
  (`engine::user_id_from_session`) and the DB `assistant_sessions.id` (a
  small lazily-populated cache keyed by `engine_session_id`, since the
  engine has no notion of the DB row). This is genuine token-level
  streaming — see the WebSocket Hub section below.

`llm/`, `local_tools/`, `mcp_client/`, and `tool_registry.rs` are the
pre-engine-swap modules: no longer reachable from `AppState`, kept compiling
standalone until issue #15 Phase 4 deletes them outright.

Configured per user via the admin SPA: `/admin/{providers,models,assistant,mcp-servers,tool-permissions}`. Provider API keys live in `llm_providers.api_key` (set through the UI, never in `.env`).

## WebSocket Hub (`src/routes/ws.rs`)

`GET /api/ws` upgrades to a single per-tab WebSocket, authenticated the same way as the rest of `/api/*` (session cookie, checked before the upgrade). `WsHub` (in `AppState.ws_hub`) is a `DashMap<user_id, Vec<mpsc::Sender<Envelope>>>` registry; each open tab holds one entry.

Frames are JSON `Envelope { topic, event, payload }`, `topic` one of `assistant | pages | files | galleries`:

- **`pages` / `files` / `galleries`** — `created`/`updated` (payload = the same summary shape the REST endpoint returns) / `deleted` (payload `{ id }`). Broadcast to **every** connected user via `WsHub::broadcast`/`broadcast_serialized` — these are shared site entities, not per-user. Published from `src/routes/api/{pages,files,galleries}.rs` after a successful create/update/delete.
- **`assistant`** — `turn_started` (`{ session_id }`), `turn_completed` (payload = full `SessionDetail`), `error` (`{ session_id, message }`). Published only to the owning user's own connections via `WsHub::publish`, from `src/ai/ws_bridge.rs`.

Server sends a WS ping every 30s; a failed send (or a client `Close` frame) drops that connection's sender from the registry. `WsHub::publish`/`broadcast` also prune any sender whose receiver has been dropped.

**Known limitation:** `assistant.*` is turn-boundary sync, not token-level streaming — the entanglement engine swap (#15) that would provide `Holly::subscribe()` deltas (`TextDelta`/`ReasoningDelta`/`ToolCallDelta`) hasn't landed. `ws_bridge.rs` is where that subscription plugs in once #15 ships.

Client side: `client/src/stores/ws.ts` owns the single connection (reconnect with exponential backoff) and topic→handler dispatch; `client/src/composables/useListSync.ts` wires `pages`/`files`/`galleries` events into the matching Pinia store's `items` list; `client/src/stores/assistant.ts` applies `assistant.*` events to session state. `client/src/App.vue` connects after login, disconnects on logout.

## Docker

```bash
docker build -t site .
docker run -e DATABASE_URL=... -p 3000:3000 site
```

`docker-compose.yml`: `db` (Postgres 17-alpine, host port 5434) + `app` (gated behind `profiles: ["full"]`).
