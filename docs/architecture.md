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
    api/                  # auth, users, pages, tags, files, galleries,
                          # menu, tokens, markdown, paths — nests
                          # ai::handlers::router() at /assistant and
                          # routes/ws.rs at /ws
    mcp.rs                # MCP JSON-RPC endpoint
    oauth.rs              # OAuth2 server (register/authorize/token/well-known)
    revision.rs
    ws.rs                 # global WebSocket hub (WsHub) + GET /api/ws upgrade
    broadcast.rs          # WsHub broadcast + PageSummary/FileSummary — one
                          # call per entity mutation, shared by the REST API,
                          # MCP server, and AI assistant tools (#25)
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
                          # policy, projection/, tools/
  repo/                   # shared CRUD/search/validation layer the REST API,
                          # MCP server, and AI tools all call into — pages,
                          # pages_search, pages_revisions, tags, files,
                          # galleries, menu, tokens, users, format (shared
                          # MCP/AI text formatters, #25)
  auth.rs config.rs design.rs files.rs
  markdown/              # mod.rs (entry + MARKDOWN_EXTENSIONS_DOC), directives.rs
                          # (tag parsing), renderer.rs (expansion pipeline),
                          # lookup.rs (file/gallery/page resolution), highlight.rs
                          # (syntect), links.rs, handlers/ (simple/media/json)
  mcp_args.rs path_util.rs state.rs templates.rs
                          # mcp_args.rs: shared tool-argument JSON parsing for
                          # the MCP server and the AI assistant's tools (#25)

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

`auth/{login,logout,me}`, `users` (`GET/POST /`, `DELETE /{id}`, `PUT /{id}/password`), `pages` CRUD + `paths` + revision restore, `tags` CRUD, `files` CRUD (multipart upload, 50 MB), `galleries` CRUD + `paths`, `menu` CRUD, `tokens` (list/create/delete), `markdown/render`, `paths/children`, and everything under `assistant/*`: `sessions` CRUD + `sessions/{id}/messages` + `sessions/{id}/messages/{message_id}/approve`, `mcp-servers` CRUD, `providers` CRUD, `models` CRUD, `permissions` CRUD (tool-permission rules).

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

The site plays both MCP roles, in two different places: it *serves* MCP to
external clients (this section — `POST /mcp`, a hand-rolled JSON-RPC 2.0
handler in `src/routes/mcp.rs`, no framework crate), and it *consumes* per-user
MCP servers on behalf of the AI assistant (`ai::mcp`'s `SiteMcp` over
`entanglement_runtime::mcp::HttpClient`, see the AI Assistant section below).

`POST /mcp` exposes JSON-RPC 2.0 with these tools (defined in `src/routes/mcp.rs`):

- **Pages:** `read_page`, `edit_page`, `search_pages` (prefix/tag/q + limit/offset), `delete_page`
- **Tags:** `list_tags`, `read_tag`, `create_tag`, `update_tag`, `delete_tag`
- **Files:** `list_files`, `create_file`, `read_file`, `update_file`, `delete_file`
- **Galleries:** `list_galleries`, `read_gallery`, `create_gallery`, `update_gallery`, `delete_gallery`

Server instructions are assembled by `server_instructions()` = `SERVER_INSTRUCTIONS_HEADER` + `MARKDOWN_EXTENSIONS_DOC` (`src/routes/mcp.rs`, `src/markdown/mod.rs`). If a private `CLAUDE` page exists (editable via admin UI / MCP), its markdown replaces the assembled default entirely — so keep that page in sync with the code. Tool/parameter descriptions live in `handle_tools_list()`.

Every mutating tool broadcasts the same `WsHub` event a REST API mutation would (`src/routes/broadcast.rs`), so a page/tag/file/gallery change made over MCP shows up live in an open admin tab. `read_page`/`search_pages`/`list_tags` render through `src/repo/format.rs`, and the pages/galleries/files/tags "empty required field" and pages "nothing to update" guards live on the `repo` mutation functions themselves (`PageSaveError`/`GallerySaveError`/`FileSaveError`/`TagSaveError`, `pages::validate_page_edit_fields`) — the same formatters, guards, and `crate::mcp_args` argument parsing are shared verbatim with the AI assistant's built-in tools (`src/ai/tools/*.rs`), so the two edges can't drift (#25).

### Markdown directives

The renderer recognizes exactly 8 HTML-tag directives — the `DIRECTIVE_NAMES` allow-list in `src/markdown/directives.rs`; any other `<tag>` passes through as raw HTML:

| Directive | Lookup keys | Other attrs | Inline body |
|---|---|---|---|
| `<page>` | `path` \| `id` | — | no |
| `<file>` | `path` \| `id` \| `hash` | — | no |
| `<image>` | `path` \| `id` \| `hash` | `alt` | no |
| `<gallery>` | `path` \| `id` | — | no |
| `<fen>` | `path` \| `id` \| `hash` \| body | `size` (`small`/`large`, `sm`/`lg`) | yes |
| `<pgn>` | `path` \| `id` \| `hash` \| body | `size`, `move` | yes |
| `<mermaid>` | `path` \| `id` \| `hash` \| body | `theme`, `size` | yes |
| `<json>` | `path` \| `id` \| `hash` \| body | `query` (jq, required), `type` (`table`) | yes |

A fenced code block with info string `mermaid` also renders as a diagram. **Single source of truth:** the human/AI-facing description is the `MARKDOWN_EXTENSIONS_DOC` const (`src/markdown/mod.rs`), reused verbatim by the MCP server instructions, the AI system prompt, and the local `site_tools` description — edit it there, not in each surface.

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
  and a session-lifecycle watcher task that evicts a session from the
  in-process "live" cache when `Holly`'s own idle-TTL sweep
  (`EngineConfig.idle_ttl`, 30 min) or a manual hibernate retires it —
  otherwise the next message to that session would skip `resume` and get a
  blank in-memory session despite intact history in `assistant_events` — and
  (issue #17) records every sub-agent session's parent link from its
  `SessionStarted` event. Two submodules (kept under the 400-line cap):
  `engine/profiles.rs` (the `researcher`/`page-writer` profile roster, below)
  and `engine/session_tree.rs` (`root_session_of`/`user_id_from_session`/
  `user_id_from_session_awaiting`, re-exported from `engine.rs` so every
  existing `crate::ai::engine::...` call site is unchanged).
  - **Sub-agents (#17):** the profile registry (`EngineConfig.profiles`) holds
    the root `build` profile plus two spawnable leaves — `researcher`
    (read-only: `web_search`/`web_fetch`/`read_page`/`search_pages`/
    `list_tags`/`list_files`/`list_galleries`) and `page-writer`
    (`read_page`/`search_pages`/`edit_page`/`create_tag`/`create_file`/
    `list_galleries`/`create_gallery`/`update_gallery`). `build`'s
    `spawnable_agents` allowlist is narrowed to exactly these two; each leaf's
    `can_spawn: Some(false)` keeps spawn depth at 1. `profile_tool_specs`
    (built via `entanglement_runtime::subagent::spawn_specs_for`) is what
    actually advertises `agent_spawn`/`agent`/`agent_poll` to a profile that
    may spawn — an empty `profile_tool_specs` entry withholds the whole
    family regardless of `may_spawn()`.
  - A sub-agent child session's own `SessionId` is a bare, unprefixed uuid
    (`entanglement_runtime::subagent::launch` mints it with no tenant
    namespacing) — `user_id_from_session` still resolves it correctly by
    walking it up to its root ancestor first (`root_session_of`, backed by a
    process-global `SESSION_PARENTS` cache fed by the watcher task above),
    *then* parsing the root's `u{user_id}:` prefix. This is what lets
    `policy.rs`'s DB-backed `PermissionResolver` — and every tool in
    `tools/*`/`mcp.rs`, all of which import the same `user_id_from_session` —
    resolve a sub-agent call against the *spawning user's* own
    `tool_permissions` rules instead of failing closed.
- `catalog.rs` — `SiteCatalog`: builds `entanglement_provider::LlmFactory`/
  `ModelResolver` closures from `llm_providers`/`llm_models`; `refresh()` is
  called after provider/model CRUD.
- `policy.rs` — `SitePolicy`: implements the engine's `PermissionResolver` +
  `GrantStore` over the existing `tool_permissions` table (same
  priority/wildcard rules as before, in `tool_permissions.rs`).
- `mcp.rs` — `SiteMcp`: per-user MCP tool discovery/routing over
  `entanglement_runtime::mcp::HttpClient` — this is the site *consuming* MCP
  servers a user has configured (`user_mcp_servers`), as opposed to the `POST
  /mcp` route below where the site itself *serves* MCP. Tools are named
  `"{server}__{tool}"`.
- `persistence.rs` — `DbSink`: the engine's `RecordSink`, appending every
  `LogRecord` to `assistant_events`; also the lazy session-resume and
  session-delete helpers.
- `projection/` — pure fold of a session's `assistant_events` rows into the
  `{role, content}` shape the admin client renders (`role` one of
  `user | assistant | tool_result | error`). Since #17, `assistant_events`
  for a session tree can hold several sessions' interleaved rows (a root plus
  any sub-agent children); `project` partitions by session, folding a child's
  own records independently and attaching them as a `content.sub_agents:
  [{agent_id, profile, task, messages}]` array on the assistant message whose
  `tool_calls` include the call that produced them. The match is *structural*,
  not positional: `InMsg::Spawn` is never persisted, so there's no direct
  field linking a `ToolCall` to the `SessionId` it produced, but
  `entanglement_runtime::subagent::launch`'s own reply text always names the
  child, and that reply is the `tool_result` paired with that call's own
  `tool_call_id` — `extract_child_session_id` recovers the uuid from it. This
  correctly handles a refused spawn (no valid uuid in its refusal text, so it
  claims nothing) and concurrent siblings in one batch (each still names its
  own child, so log order between them doesn't matter).
- `tools/` — the built-in (non-MCP) tool vocabulary, ported to
  `entanglement_runtime::tools::Tool`. A curated subset of the site API (not
  full CRUD): pages `read`/`search`/`edit`/`delete`, tags `list`/`create`,
  files `list`/`create`, galleries `list`/`create`/`update`, plus
  `web_search`/`web_fetch`.
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
  `SessionHibernated`, plus — #17 — a sub-agent child's own `SessionStarted`)
  to `WsHub` as `assistant.*` envelopes, resolving each event's `SessionId` to
  both the owning `user_id` and the DB `assistant_sessions.id` off its
  **root** ancestor (a lazily-populated cache keyed by `engine_session_id`,
  since the engine has no notion of the DB row, and a sub-agent child is never
  itself a DB row). Root resolution keeps its own `local_parents` map fed
  synchronously from this same ordered broadcast, rather than trusting
  `engine::root_session_of`'s process-global cache alone — that cache is
  written by a *different*, independently-scheduled subscriber task, so
  resolving a child's very own `SessionStarted` off it would race; falls back
  to the global cache only for a child whose `SessionStarted` predates this
  subscription. A sub-agent event's payload also carries `agent_session_id`
  (the child's own engine `SessionId`) so the client can nest it under the
  right root turn; a root-level event carries no such field, keeping the
  envelope shape unchanged for any session that never spawns a sub-agent.
  This is genuine token-level streaming — see the WebSocket Hub section
  below.

Configured per user via the admin SPA: `/admin/{providers,models,assistant,mcp-servers,tool-permissions}`. Provider API keys live in `llm_providers.api_key` (set through the UI, never in `.env`).

## WebSocket Hub (`src/routes/ws.rs`)

`GET /api/ws` upgrades to a single per-tab WebSocket, authenticated the same way as the rest of `/api/*` (session cookie, checked before the upgrade). `WsHub` (in `AppState.ws_hub`) is a `DashMap<user_id, Vec<mpsc::Sender<Envelope>>>` registry; each open tab holds one entry.

Frames are JSON `Envelope { topic, event, payload }`, `topic` one of `assistant | pages | files | galleries | tags`:

- **`pages` / `files` / `galleries` / `tags`** — `created`/`updated` (payload = the same summary shape the REST endpoint returns) / `deleted` (payload `{ id }`). Broadcast to **every** connected user via `WsHub::broadcast`/`broadcast_serialized` — these are shared site entities, not per-user. Published from the shared `src/routes/broadcast.rs` helpers, called after a successful create/update/delete from all three mutating edges — `src/routes/api/{pages,files,galleries,tags}.rs`, `src/routes/mcp.rs`, and `src/ai/tools/*.rs` — so a mutation over MCP or by the AI assistant broadcasts the same event a REST API mutation would (#25).
- **`assistant`** — real, token-level streaming straight off `agent_engine.holly.subscribe()` (`src/ai/ws_bridge.rs`), published only to the owning user's own connections via `WsHub::publish`. `event` is the forwarded `OutEvent`'s own `"kind"` tag and `payload` is that event's JSON shape (`entanglement_core::OutEvent` already derives `Serialize`) plus a spliced-in `db_session_id` (the `assistant_sessions.id` the engine's root `SessionId` resolves to, cached per session): `status` (`AgentState`: idle/thinking/waiting_approval/waiting_answer/done/error), `text_delta`/`reasoning_delta` (incremental text), `tool_call_delta` (incremental tool-input fragment), `tool_call` (display-only, full call), `tool_request` (needs approval — approve/reject the same way as an existing message's tool call, `POST .../messages/{any}/approve`, since the engine no longer keys approvals by message id), `tool_output`, `done`, `error`, `session_hibernated`, and (#17) a sub-agent child's own `session_started` (`{session, profile, parent}`, `researcher`/`page-writer`). Any event belonging to a sub-agent child also carries `agent_session_id` (the child's own engine `SessionId`) so the client can render it nested under the spawning turn instead of the root's own top-level stream.

Server sends a WS ping every 30s; a failed send (or a client `Close` frame) drops that connection's sender from the registry. `WsHub::publish`/`broadcast` also prune any sender whose receiver has been dropped.

Client side: `client/src/stores/ws.ts` owns the single connection (reconnect with exponential backoff) and topic→handler dispatch; `client/src/composables/useListSync.ts` wires `pages`/`files`/`galleries`/`tags` events into the matching Pinia store's `items` list; `client/src/stores/assistantLiveTurns.ts` (composed into `stores/assistant.ts`) accumulates `assistant.*` deltas into a `live: LiveTurn | null` (the root turn) and, since #17, `liveSubAgents: Record<string, LiveSubAgentTurn>` keyed by `agent_session_id` — a sub-agent's events carry that field instead of belonging to the root, and it is tracked independently of `live` since a detached child keeps streaming after the root's own turn has already settled. Both are rendered by `AssistantView.vue` as in-progress bubbles (`LiveSubAgentTurn.vue`/`LiveToolCallList.vue`), and on a turn's own `done`/`error`/`session_hibernated` the matching entry clears and the session refetches over REST for the authoritative message list — reusing `ai::projection::project`'s fold (including its `content.sub_agents` nesting, rendered by the self-recursive `AssistantMessageContent.vue`) rather than re-implementing it in TypeScript. `client/src/App.vue` connects after login, disconnects on logout.

## Docker

```bash
docker build -t site .
docker run -e DATABASE_URL=... -p 3000:3000 site
```

`docker-compose.yml`: `db` (Postgres 17-alpine, host port 5434) + `app` (gated behind `profiles: ["full"]`).
