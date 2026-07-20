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
    public/               # catch-all, images.rs (file serving), search,
                          # sitemap, tags
    api/                  # auth, users, pages, tags, files, galleries,
                          # menu, tokens, markdown, paths — nests
                          # ai::handlers::router() at /assistant and
                          # routes/ws.rs at /ws
    mcp/                  # MCP JSON-RPC endpoint: mod.rs (router/dispatch),
                          # rpc.rs (JSON-RPC envelope + parse_args),
                          # instructions.rs (server_instructions +
                          # handle_tools_list), pages.rs/tags.rs/files.rs/
                          # galleries.rs (one tool family per file)
    oauth/                # OAuth2 server: mod.rs (router + base_url),
                          # handlers.rs (authorize/token Axum handlers),
                          # security.rs (PKCE verify, code/token issuance +
                          # refresh, authenticate_mcp — no Axum extractors),
                          # metadata.rs (register + well-known discovery)
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
  migration/              # m_001 … m_029
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

llm_providers       id, label, kind (anthropic|ollama|gemini|openai), api_key?,
                    base_url? (required for ollama/openai — openai is the
                    generic OpenAI-compat kind covering z.ai/OpenAI/any
                    compatible proxy, api_key optional for keyless local
                    proxies), concurrency? (m_026 — max in-flight requests to
                    this endpoint), rpm? (m_026 — requests/minute budget);
                    both `None` fall back to entanglement_provider's client
                    defaults (ADR-0111)
llm_models          id, provider_id, label, model wire-id, is_default,
                    context_window? (m_025 — tokens; fed to
                    ResolvedModel::context_window, #40), supports_temperature
                    (default true), supports_reasoning_effort, supports_thinking
                    (both default false; m_029) — gate the matching
                    GenerationParams knob per model so an unsupported one is
                    rejected with 400 instead of reaching the provider, #53
assistant_sessions  id, user_id, title, provider/model snapshots, model_id?,
                    enabled_mcp_server_ids JSONB (m_018),
                    engine_session_id? unique (m_023 — the engine's root
                    SessionId string, "u{user_id}:{uuid}"; nullable since
                    pre-engine-swap rows never get one back; repointed to a
                    fresh successor session id by a manual /compact, #40),
                    temperature?, reasoning_effort? (m_027), max_output_tokens?,
                    thinking_budget_tokens? (m_028) — session-level
                    `GenerationParams` overrides, #42; `None` leaves that knob
                    at the model's own default), agent_profile (m_027,
                    default `"build"` — the engine profile the session runs
                    under, `"build"`/`"researcher"`/`"page-writer"`),
                    timestamps
assistant_events    id, root_session_id (engine SessionId string, not a DB FK —
                    the engine has no notion of assistant_sessions.id),
                    payload JSONB (a serialized entanglement_runtime
                    LogRecord), created_at (m_023). Event-sourced log that
                    replaced the old per-message assistant_messages table;
                    `ai::projection::project` folds a session's rows into the
                    {role, content} shape the admin client renders.
user_mcp_servers    id, user_id, name, url, enabled, forward_user_token,
                    headers JSON, capabilities JSON (m_024 — raw remote tool
                    name -> read|write|call, #39/ADR-0117 fan-out)
tool_permissions    id, user_id, name pattern, effect (allow|deny|prompt),
                    priority — name is a literal tool name, `*`, a
                    capability key (read|write|call), or a scoped form
                    tool(argpattern) / tool{workdirpattern} (#39)
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

`auth/{login,logout,me}`, `users` (`GET/POST /`, `DELETE /{id}`, `PUT /{id}/password`), `pages` CRUD + `paths` + revision restore, `tags` CRUD, `files` CRUD (multipart upload, 50 MB), `galleries` CRUD + `paths`, `menu` CRUD, `tokens` (list/create/delete), `markdown/render`, `paths/children`, and everything under `assistant/*`: `sessions` CRUD + `sessions/{id}/messages` + `sessions/{id}/messages/{message_id}/approve` + `sessions/{id}/compact` (manual context compaction, #40), `mcp-servers` CRUD, `providers` CRUD, `models` CRUD (`context_window` field, #40), `permissions` CRUD (tool-permission rules).

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
handler in `src/routes/mcp/` — `mod.rs` wires the route and dispatches by
method/tool name, `rpc.rs` holds the JSON-RPC envelope types + `parse_args`,
`instructions.rs` holds the static `initialize`/`tools/list` content, and
`pages.rs`/`tags.rs`/`files.rs`/`galleries.rs` hold one tool family each as
plain `async fn`s callable without the Axum router; no framework crate), and
it *consumes* per-user MCP servers on behalf of the AI assistant (`ai::mcp`'s
`SiteMcp` over `entanglement_runtime::mcp::HttpClient`, see the AI Assistant
section below).

`POST /mcp` exposes JSON-RPC 2.0 with these tools (defined in `src/routes/mcp/{pages,tags,files,galleries}.rs`):

- **Pages:** `page_read`, `page_edit`, `page_search` (prefix/tag/q + limit/offset), `page_delete`
- **Tags:** `tag_list`, `tag_read`, `tag_create`, `tag_update`, `tag_delete`
- **Files:** `file_list`, `file_create`, `file_read` (`include_content` returns the file's text for text-ish mimetypes — plain text, JSON, PGN, mermaid, FEN, per `files_repo::is_text_content`), `file_update` (path/description, plus optional `mimetype`/`data`/`data_base64` to replace the stored bytes in place — issue #56, so a bad upload is repairable instead of unrecoverable), `file_delete`
- **Galleries:** `gallery_list`, `gallery_read`, `gallery_create`, `gallery_update`, `gallery_delete`

Tool names follow a `<resource>_<operation>` convention (issue #61); `web_search`/`web_fetch` (below) are the exception, already resource-first.

Server instructions are assembled by `server_instructions()` = `SERVER_INSTRUCTIONS_HEADER` + `MARKDOWN_EXTENSIONS_DOC` (`src/routes/mcp/instructions.rs`, `src/markdown/mod.rs`). If a private `CLAUDE` page exists (editable via admin UI / MCP), its markdown replaces the assembled default entirely — so keep that page in sync with the code. Tool/parameter descriptions live in `handle_tools_list()` (`src/routes/mcp/instructions.rs`).

Every mutating tool broadcasts the same `WsHub` event a REST API mutation would (`src/routes/broadcast.rs`), so a page/tag/file/gallery change made over MCP shows up live in an open admin tab. `page_read`/`page_search`/`tag_list` render through `src/repo/format.rs`, and the pages/galleries/files/tags "empty required field" and pages "nothing to update" guards live on the `repo` mutation functions themselves (`PageSaveError`/`GallerySaveError`/`FileSaveError`/`TagSaveError`, `pages::validate_page_edit_fields`) — the same formatters, guards, and `crate::mcp_args` argument parsing are shared verbatim with the AI assistant's built-in tools (`src/ai/tools/*.rs`), so the two edges can't drift (#25).

### Markdown directives

The renderer recognizes exactly 8 HTML-tag directives — the `DIRECTIVE_NAMES` allow-list in `src/markdown/directives.rs`; any other `<tag>` passes through as raw HTML:

| Directive | Lookup keys | Other attrs | Inline body |
|---|---|---|---|
| `<page>` | `path` \| `id` | — | no |
| `<file>` | `path` \| `id` \| `hash` | — | no |
| `<image>` | `path` \| `id` \| `hash` | `alt` | no |
| `<gallery>` | `path` \| `id` | — | no |
| `<fen>` | `path` \| `id` \| `hash` \| body | `size` (`small`/`large`, `sm`/`lg`) | yes |
| `<pgn>` | `path` \| `id` \| `hash` \| body | `size` (`small`/`large`, `sm`/`lg`), `move` | yes |
| `<mermaid>` | `path` \| `id` \| `hash` \| body | `theme`, `size` (`small`/`large`, `sm`/`lg`) | yes |
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
  `SessionStarted` event. `EngineConfig.auto_compact = true` (#40, explicit —
  matches the library default): on context overflow the turn loop
  LLM-summarizes the oldest history in place (ADR-0103) instead of a lossy
  placeholder-prune. `EngineConfig.context_window` is seeded from the
  catalog's default model (`catalog.default_model()`) so a freshly-spawned
  session budgets sensibly before any `SetModel` narrows it to the session's
  actual pinned model. Three submodules (kept under the 400-line cap):
  `engine/profiles.rs` (the `researcher`/`page-writer` profile roster, below),
  `engine/session_tree.rs`
  (`root_session_of`/`user_id_from_session`/`user_id_from_session_awaiting`,
  re-exported from `engine.rs` so every existing `crate::ai::engine::...` call
  site is unchanged), and `engine/prompt_cache.rs` (the system-prompt
  read-and-refresh loop).
  - **Tool registry (issue #38):** `spawn` builds the local (built-in,
    non-MCP) `ToolRegistry` once and wraps it in a `SharedRegistry`
    (`entanglement_runtime::tools::SharedRegistry`, an
    `Arc<RwLock<ToolRegistry>>` 0.3 added for exactly this) — the same handle
    is handed to `tool_runner::spawn_tool_executor_with_policy` and to
    `SiteMcp`. There is no seed-at-spawn step, no periodic rebuild, and no
    executor swap: `SiteMcp` mutates the registry in place as users'
    MCP servers connect/disconnect (see `mcp.rs` below), and the one executor
    spawned at boot keeps running for the process lifetime.
  - **Sub-agents (#17):** the profile registry (`EngineConfig.profiles`) holds
    the root `build` profile plus two spawnable leaves — `researcher`
    (read-only: `web_search`/`web_fetch`/`page_read`/`page_search`/
    `tag_list`/`file_list`/`gallery_list`) and `page-writer`
    (`page_read`/`page_search`/`page_edit`/`tag_create`/`file_create`/
    `gallery_list`/`gallery_create`/`gallery_update`). `build`'s
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
    `user_id_from_session_awaiting` (used wherever the caller has no ordered
    stream of its own to fall back on, e.g. `policy.rs`) retries a bare lookup
    a few times to close a TOCTOU window: `SESSION_PARENTS` is written by this
    watcher's own `holly.subscribe()`r, an independent broadcast subscriber
    racing against whoever else needs the same child's link. Reassessed for
    #43 against entanglement 0.3.0's cascading `resume` (ADR-0112, which
    re-materializes a root's whole spawn sub-tree and re-announces each
    child's `SessionStarted` exactly like a live spawn): this cache's resume
    population needs no extra code — the same generic watcher already covers
    a cascaded child — but the retry race is *not* eliminated, only widened to
    also cover a resume-reconstituted child's re-announced `SessionStarted`,
    not just a freshly live-spawned one. Neither `SESSION_PARENTS` nor the
    retry is a library-guaranteed problem this site can drop.
- `catalog.rs` — `SiteCatalog`: builds `entanglement_provider::LlmFactory`/
  `ModelResolver` closures from `llm_providers`/`llm_models`; `refresh()` is
  called after provider/model CRUD. `ModelResolver` populates
  `ResolvedModel::context_window` from each model row's own `context_window`
  (#40), so a live `SetModel`/session resume budgets the turn loop's
  overflow handling against the model's real window instead of the engine's
  generic fallback (`entanglement_core::context::CONTEXT_LIMIT_TOKENS`).
  `build_factory` also threads each row's `concurrency`/`rpm` (m_026) into the
  `*_factory` calls (ADR-0111 in `entanglement_provider`, #41): every session
  built from that row shares one per-endpoint `HttpClient` state keyed by
  (base URL, api key), so `concurrency` caps simultaneously in-flight
  requests (held across the whole streamed turn — the storm guard for many
  spawned sub-agents) and `rpm` sets the adaptive pacing gate; a 429 backs
  the gate off and a success relaxes it. Both are nullable — `None` falls
  back to the library's own client defaults (3 concurrent / 50 rpm). The
  values are also exposed on `CatalogModel.concurrency`/`.rpm` for
  introspection, even though they're already baked into the `llm_factory`
  closure. `refresh()` builds a **fresh** `HttpClient` every call rather than
  reusing one long-lived instance — `entanglement_provider`'s `HttpClient`
  locks in an endpoint's rpm/concurrency on that endpoint's *first* request
  and ignores later values for the same key, so a stale client would make an
  admin's `concurrency`/`rpm` edit silently have no effect once any turn had
  already gone through that provider. `generation_resolver()` builds the
  `GenerationResolver` closure for `EngineConfig.generation_resolver` (#42) —
  the generation-parameter analogue of `model_resolver`, resolving a named
  agent profile's *persisted* generation override (ADR-0094). This site has
  no such per-profile store (unlike the model pin, which `engine/profiles.rs`
  bakes straight into `AgentProfile.provider`/`.model`): generation knobs are
  set live per-*session* instead, via `InMsg::SetGeneration`
  (`handlers/sessions`, below), so the closure always returns `None` — wired
  for parity, not because anything populates it yet.
- `policy.rs` — `SitePolicy`: implements the engine's `PermissionResolver` +
  `GrantStore` over the `tool_permissions` table, via `tool_permissions.rs`
  (#39). Extracts a call's scoping argument with its own `permission_arg`
  (this site's tool vocabulary, not the coding-agent's) and resolves through
  `entanglement_core::PermissionProfile::resolve_scoped`.
- `mcp.rs` — `SiteMcp`: per-user MCP tool discovery/routing over
  `entanglement_runtime::mcp::HttpClient` — this is the site *consuming* MCP
  servers a user has configured (`user_mcp_servers`), as opposed to the `POST
  /mcp` route below where the site itself *serves* MCP. Tools are named
  `"{server}__{tool}"`. Connecting to a user's server is bounded by a 10s
  `CONNECT_TIMEOUT` (issue #28). Holds the same `SharedRegistry` handle
  `engine.rs` wraps at spawn (issue #38): every time `routes_for_user`
  (re)connects a user's servers — a cold cache or a 60s TTL expiry —
  `register_routes` registers each newly discovered `"{server}__{tool}"`
  identity into that registry, so it's dispatchable as soon as it's
  discovered, with no seed-at-spawn step and no periodic rebuild.
  `invalidate_user` (called by `ai::handlers::mcp_servers`' CRUD handlers
  after a row changes) is the deregistering counterpart — it drops the user's
  cached routes and unregisters any identity no other currently-cached user
  still has. `SiteMcp` keeps a weak self-handle (`self_ref`, set right after
  construction) so it can hand `McpRoutedTool` the `Arc<SiteMcp>` it needs
  from a `&self` method.
- `persistence.rs` — `DbSink`: the engine's `RecordSink`, appending every
  `LogRecord` to `assistant_events` behind a bounded channel + writer task;
  also the lazy session-resume and session-delete helpers. A record shed under
  sink backlog (channel full) is tallied and turned into a `LogPayload::Gap`
  tombstone by a periodic flush, same as `entanglement_runtime`'s own
  broadcast-lag path — and `resume_session` (issue #28) no longer hard-refuses
  a session with a detected gap; it resumes from the intact prefix strictly
  before the gap (`truncate_at_gap`), so a session stays resumable forever
  instead of every future `ensure_live` failing. `resume_session` passes
  `assistant_events`' whole root file (root + any sub-agent children, since
  they share one `root_session_id`) to `Holly::resume` in one call —
  entanglement 0.3.0's `resume` cascades over the *whole* spawn sub-tree
  itself (ADR-0112), re-materializing a child that was still live as of where
  the log stopped, so no per-child loop is needed here.
  `handlers/sessions/turn/collect.rs`'s `send_and_collect` builds its own response
  from the `LogRecord`s it just observed rather than re-reading
  `assistant_events` after a turn settles — reassessed for #43 and unrelated
  to `entanglement_runtime`'s own guarantees either way: it exists solely
  because `DbSink`'s async writer task gives no read-your-writes guarantee at
  the instant this handler observes e.g. `Done`.
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
  own child, so log order between them doesn't matter). `content.is_error` on
  a `tool_result` is a text-prefix heuristic (`looks_like_tool_error`), not a
  structural flag — `OutEvent::ToolOutput` carries none, re-checked against
  entanglement-core 0.3.0 for #43 and still true.
- `tools/` — the built-in (non-MCP) tool vocabulary, ported to
  `entanglement_runtime::tools::Tool`. A curated subset of the site API (not
  full CRUD): pages `read`/`search`/`edit`/`delete`, tags `list`/`create`,
  files `list`/`create`/`read`/`update`/`delete` (issue #56 — `file_update`
  can replace a file's stored bytes/mimetype in place, and `file_read` can
  return its text content, so a bad upload is repairable instead of orphaned),
  galleries `list`/`create`/`update`, plus `web_search`/`web_fetch`. Tool
  names follow the `<resource>_<operation>` convention (issue #61).
- `tool_permissions.rs` — the allow/deny/prompt rule evaluator `policy.rs`
  wraps (#39): a user's rows (ordered `priority DESC, id DESC` — ascending
  precedence, so `PermissionProfile::resolve_scoped`'s last-match-wins
  reproduces the old `priority ASC, id ASC` first-match-wins semantics) build
  an `entanglement_core::PermissionProfile`. A rule name is a literal tool
  name, `*`, a capability key (`read`/`write`/`call`, `CAPABILITIES` — this
  site's own tool vocabulary, since the coding-agent's built-in capability
  table in `entanglement_runtime::tool_names` is wired to tool names this
  site doesn't have), or a scoped form `tool(argpattern)` /
  `tool{workdirpattern}`. `expand_capabilities` fans a capability key out to
  its member tools (mirrors the library's own — private —
  `agents::expand_capabilities`) plus any MCP tool a server's `capabilities`
  annotation maps to it (ADR-0117, `mcp_capability_index` reads
  `user_mcp_servers.capabilities`). No site tool exposes a working directory
  yet, so a `tool{pattern}` rule is stored and matched like any other but
  never fires (`SitePolicy` always passes `workdir = None`).
- `handlers/` — `/api/assistant/*`: `sessions/` (CRUD + `messages`/`approve`,
  which drive a turn through `Holly` and project `assistant_events` on the
  way out, plus `compact.rs`'s `sessions/{id}/compact`, #40 — see below),
  `mcp_servers.rs`, `providers.rs`, `models.rs` (`context_window` field,
  #40), `permissions.rs`.
  - **Live model/generation/profile switching (`sessions/mod.rs`,
    `sessions/mutate/generation.rs`, #42):** `POST /sessions` and
    `PATCH /sessions/{id}` accept optional `temperature`/`reasoning_effort`/
    `max_output_tokens`/`thinking_budget_tokens` (m_028) /`agent_profile`
    alongside the existing `model_id` — every one of them is a live,
    no-restart switch, not just a row update. `create` sends `InMsg::SetModel`
    (as today), then, if given, `InMsg::SetAgent` and `InMsg::SetGeneration`
    on the freshly-spawned session. `update` diffs the incoming fields
    against the row, resumes the session once (`ensure_live`, same guard the
    existing `model_id` path already used) if *any* of `model_id`/
    `agent_profile`/`temperature`/`reasoning_effort`/`max_output_tokens`/
    `thinking_budget_tokens` changed, then sends `SetModel`/`SetAgent`/
    `SetGeneration` for whichever actually did — in that order, though it's
    not load-bearing here since neither built-in profile
    (`engine/profiles.rs`) pins a model. `reasoning_effort` is validated
    against `low|medium|high`, `max_output_tokens`/`thinking_budget_tokens`
    against `Some(0)` being rejected as meaningless, and `agent_profile`
    against `engine::SWITCHABLE_PROFILES` (`build`/`researcher`/
    `page-writer`) at the API boundary — `entanglement_core` itself imposes
    no reachability gate on a direct `SetAgent` — so an unknown/invalid value
    is rejected `400` before any DB write. All four generation knobs persist
    onto the session row verbatim as partial overrides (an omitted field
    leaves the column untouched, SeaORM `NotSet`, mirroring `title`/
    `model_id`'s existing convention) — the row is a display cache of the
    caller's intent, not the engine's merged state; the engine's own
    `Session::generation` is the source of truth `OutEvent::GenerationChanged`
    reports back over the WS bridge. `create`/`update` themselves live in
    their own `sessions/mutate/create.rs`/`sessions/mutate/update.rs` files
    (`mutate.rs`'s own 400-line cap, #54) — `mutate.rs` keeps only what both
    share (`apply_live_changes`, the MCP-id/model-resolution helpers).
  - **A model switch preserves existing generation overrides (`sessions/
    mutate/generation.rs`'s `generation_after_model_switch`/
    `carry_forward_generation`, #54):** `entanglement-core`'s `rebind()`
    rebuilds the live session's `generation` from the `ModelResolver`'s
    `ResolvedModel::generation`, which `SiteCatalog::model_resolver` always
    resolves to `None` (the resolver has no session handle to read the prior
    value from) — so a model-only `PATCH` (no generation fields in the
    request) would otherwise silently wipe every knob. `update` re-derives
    the row's existing overrides — falling back to whichever knobs this call
    didn't explicitly change — and resends them via `SetGeneration`
    immediately after `SetModel`, silently dropping any knob the new model
    doesn't support (#53) rather than rejecting the whole switch or
    resurrecting it once the session switches back.
  - **Manual compaction (`handlers/sessions/compact.rs`, #40):** drives
    `entanglement_core`'s copy-on-write `InMsg::Oneshot { op: "compact" }` on
    the session's current (source) engine session, which reports an
    LLM-generated summary via `OutEvent::Compacted { auto: false, .. }`
    without mutating the source (ADR-0101). The handler then forks the
    summary into a fresh successor session (`InMsg::Spawn` with
    `predecessor: Some(source)`, a root — not a child — so closing the source
    doesn't cascade onto it, ADR-0110), re-pins the successor's model
    (`InMsg::SetModel`, best-effort — the successor's own seeded first turn
    already ran under the engine default by the time this lands, since
    `SetModel` is stashed behind a live turn), and retires the source
    (`InMsg::CloseSession`). The `assistant_sessions` row keeps its id/title;
    only `engine_session_id` repoints to the successor, so `GET
    /sessions/{id}` (and every other DB-id-keyed handler) transparently
    follows the fork. The source's own `assistant_events` log is left
    intact but unreachable from the DB row (ADR-0101: "the original stays
    idle, intact, independently resumable"). Broadcasts a `compacted` event
    over the `assistant` WS topic (see below) so another open tab on the
    session notices and refetches.
- `ws_bridge.rs` — a single process-wide task subscribing to
  `agent_engine.holly.subscribe()` (issue #16): forwards the engine's
  content/lifecycle `OutEvent`s (`Status`, `TextDelta`, `ReasoningDelta`,
  `ToolCallDelta`, `ToolCall`, `ToolRequest`, `ToolOutput`, `Done`, `Error`,
  `SessionHibernated`, plus — #17 — a sub-agent child's own `SessionStarted`,
  and — #42 — `ModelChanged`/`GenerationChanged`/`AgentChanged`, so a live
  `/model`/generation/profile switch made from *another* tab is visible
  without a manual reload) to `WsHub` as `assistant.*` envelopes, resolving
  each event's `SessionId` to
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

- **`pages` / `files` / `galleries` / `tags`** — `created`/`updated` (payload = the same summary shape the REST endpoint returns) / `deleted` (payload `{ id }`). Broadcast to **every** connected user via `WsHub::broadcast`/`broadcast_serialized` — these are shared site entities, not per-user. Published from the shared `src/routes/broadcast.rs` helpers, called after a successful create/update/delete from all three mutating edges — `src/routes/api/{pages,files,galleries,tags}.rs`, `src/routes/mcp/{pages,tags,files,galleries}.rs`, and `src/ai/tools/*.rs` — so a mutation over MCP or by the AI assistant broadcasts the same event a REST API mutation would (#25).
- **`assistant`** — real, token-level streaming straight off `agent_engine.holly.subscribe()` (`src/ai/ws_bridge.rs`), published only to the owning user's own connections via `WsHub::publish`. `event` is the forwarded `OutEvent`'s own `"kind"` tag and `payload` is that event's JSON shape (`entanglement_core::OutEvent` already derives `Serialize`) plus a spliced-in `db_session_id` (the `assistant_sessions.id` the engine's root `SessionId` resolves to, cached per session): `status` (`AgentState`: idle/thinking/waiting_approval/waiting_answer/done/error), `text_delta`/`reasoning_delta` (incremental text), `tool_call_delta` (incremental tool-input fragment), `tool_call` (display-only, full call), `tool_request` (needs approval — approve/reject the same way as an existing message's tool call, `POST .../messages/{any}/approve`, since the engine no longer keys approvals by message id), `tool_output`, `done`, `error`, `session_hibernated`, and (#17) a sub-agent child's own `session_started` (`{session, profile, parent}`, `researcher`/`page-writer`). Any event belonging to a sub-agent child also carries `agent_session_id` (the child's own engine `SessionId`) so the client can render it nested under the spawning turn instead of the root's own top-level stream. `compacted` (#40) is the one `assistant.*` event *not* forwarded by `ws_bridge.rs` — `handlers/sessions/compact.rs` publishes it directly once a manual compaction's fork/retire completes, carrying the real `OutEvent::Compacted` shape (`summary`, `kept`, `auto: false`) plus `db_session_id` and `successor_session_id`, so another open tab on the session notices its `engine_session_id` moved and refetches.

Server sends a WS ping every 30s; a failed send (or a client `Close` frame) drops that connection's sender from the registry. `WsHub::publish`/`broadcast` also prune any sender whose receiver has been dropped.

Client side: `client/src/stores/ws.ts` owns the single connection (reconnect with exponential backoff) and topic→handler dispatch; `client/src/composables/useListSync.ts` wires `pages`/`files`/`galleries`/`tags` events into the matching Pinia store's `items` list; `client/src/stores/assistantLiveTurns.ts` (composed into `stores/assistant.ts`) accumulates `assistant.*` deltas into a `live: LiveTurn | null` (the root turn) and, since #17, `liveSubAgents: Record<string, LiveSubAgentTurn>` keyed by `agent_session_id` — a sub-agent's events carry that field instead of belonging to the root, and it is tracked independently of `live` since a detached child keeps streaming after the root's own turn has already settled. Both are rendered by `AssistantView.vue` as in-progress bubbles (`LiveSubAgentTurn.vue`/`LiveToolCallList.vue`), and on a turn's own `done`/`error`/`session_hibernated`/`compacted` (#40) the matching entry clears and the session refetches over REST for the authoritative message list — reusing `ai::projection::project`'s fold (including its `content.sub_agents` nesting, rendered by the self-recursive `AssistantMessageContent.vue`) rather than re-implementing it in TypeScript. `client/src/App.vue` connects after login, disconnects on logout. `AssistantView.vue`'s header also has a "Compact" button (`assistant.compactSession`) that `POST`s `sessions/{id}/compact` and swaps in the returned successor-session detail directly.

## Docker

```bash
docker build -t site .
docker run -e DATABASE_URL=... -p 3000:3000 site
```

`docker-compose.yml`: `db` (Postgres 17-alpine, host port 5434) + `app` (gated behind `profiles: ["full"]`).
