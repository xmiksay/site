# site — architecture

Deep reference for the personal site. The brief [`.claude/CLAUDE.md`](../.claude/CLAUDE.md) carries the overview, stack, build/run, environment, and conventions, and points here. **Keep this current:** when a change adds/removes/renames a module, route, entity, env var, or MCP tool, update the matching section here in the same change.

## Project Structure

```
src/
  bin/
    site_server.rs        # HTTP server, port 3000
    site_migration.rs     # Migration CLI (up/down/fresh/status)
    site_cli.rs           # create-user, change-password, storage migrate,
                          # design push, design contract
  routes/
    public/               # catch-all, export.rs (GET /{*path}?format=...,
                          # #67), images.rs (file serving), assets.rs
                          # (/assets/*), preview.rs (draft preview: Look,
                          # banner, template error page), search,
                          # sitemap, tags
    api/                  # auth, users, pages, tags, files, galleries,
                          # menu, tokens, markdown, paths, export.rs
                          # (GET /export/pages/{id}?format=..., #67) — nests
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
  migration/              # m_001 … m_032
  ai/                     # config, handlers, tool_permissions, ws_bridge/ —
                          # plus the entanglement-core/-runtime engine
                          # adapters: engine, catalog, mcp, persistence,
                          # policy, projection/, tools/
  repo/                   # shared CRUD/search/validation layer the REST API,
                          # MCP server, and AI tools all call into — pages,
                          # pages_search, pages_revisions, tags, files,
                          # galleries, menu, tokens, users, format (shared
                          # MCP/AI text formatters, #25)
  export/                 # thin client for the remote mdcast-server (0.4) —
                          # mod.rs: ExportError + build_client (the injected
                          # reqwest client is where timeouts live) plus
                          # sanitize_filename, shared by both export routes;
                          # bundle.rs: build_bundle, declaring every asset a
                          # render references (eager bridge SVGs + design
                          # templates, digest-only lazy storage images);
                          # render.rs: ExportFormat + render_page, the
                          # render entrypoint the public/admin export routes
                          # call (#67), and load_brand (#68)
  design/                # mod.rs: DesignStore (DESIGN_DIR → stored → baked);
                          # stored.rs: published design/… objects, validated
                          # reload, validate_for_publish; draft.rs: the
                          # shared design-draft/ (init, edit, discard,
                          # change list, mirror); publish.rs: publish,
                          # design-history/ snapshots, restore, crash
                          # recovery; push.rs: `site_cli design push`;
                          # view.rs: Resolve (what a request renders
                          # with) + the cached DraftSite for preview
  storage/               # mod.rs: Storage (db | fs | s3 over object_store) —
                          # put_blob/get_blob/get_blob_stream by sha256;
                          # objects.rs: keyed get/put/delete/list on every
                          # backend; db_objects.rs: their `storage_objects`
                          # rows on db; config.rs: STORAGE_KIND/STORAGE_DIR/S3_*;
                          # migrate.rs: `site_cli storage migrate`
  auth.rs config.rs files.rs
  markdown/              # mod.rs (entry + MARKDOWN_EXTENSIONS_DOC), directives.rs
                          # (tag parsing), renderer.rs (expansion pipeline),
                          # lookup.rs (file/gallery/page resolution), highlight.rs
                          # (syntect), links.rs, handlers/ (simple/media/json)
  templates/             # mod.rs: Templates (frozen/live environments,
                          # timeformat, strict_environment, DesignLoader);
                          # context.rs: typed render contexts, one per
                          # template/partial; contract.rs (+ contract_intro.md):
                          # the design contract generator; samples.rs: example
                          # contexts; smoke.rs: strict smoke render (#117)
  mcp_args.rs path_util.rs state.rs
                          # mcp_args.rs: shared tool-argument JSON parsing for
                          # the MCP server and the AI assistant's tools (#25)

client/                   # Vue 3 SPA
  src/  dist/             # dist/ is embedded into the binary

design/                   # Baked default design bundle (via rust-embed)
  templates/              # rendered by the template engine
  assets/                 # served statically under /assets/* (css/ js/ img/)
  mdcast/                 # export brand config + template overrides (#68) —
                          # see "Export (mdcast)" below
```

Design/template resolution (see `src/design/`, `DesignStore`):
`DESIGN_DIR` (dev-only, live) → storage overrides (`design/…` keys) → baked
`design/` → not found. All override layers mirror the bundle layout
(`templates/`, `assets/{css,js,img}`, `mdcast/`); only paths under those three
roots are served or accepted. See [Design overrides](#design-overrides).

Templates (`src/templates/`, `Templates`) sit on top of the same `DesignStore`:
release builds compile every template once at startup (frozen, shared); debug
builds rebuild the environment from the assets on each render (live reload).

### Template contract

Every template is rendered with a typed context struct
(`src/templates/context.rs`, `#[derive(Serialize, JsonSchema)]`): `Layout`
(flattened into each page context; `404.html`/`base.html` get it alone),
`PathPageContext`, `PageSearchContext`, and one `*Partial` per directive
partial under `templates/markdown/`. Every field is always serialized (absent =
`none`), so templates render under `UndefinedBehavior::Strict`.

`templates::contract` turns the structs (doc comments = descriptions) plus the
conventions (resolution order, `timeformat`, URLs) into the
**[design contract](design-contract.md)** — `markdown()` (tables + an example
context per template) and `json_schema()` / `json_schema_text()` (one JSON
Schema document: `templates.<name>` + shared `$defs`); `TEMPLATES` is the
registry (name, kind, schema, example contexts). `docs/design-contract.md` and
`docs/design-contract.schema.json` are generated by `make contract`
(`site_cli design contract [--schema]`); a unit test fails when either drifts
from the code, another when a baked template is missing from `TEMPLATES`.

`templates::smoke::smoke_render(db, storage, load)` renders a design given as
a `DesignLoader` (`templates/…` path → bytes: a `DesignStore`, a draft, …)
through `strict_environment`: `404.html`, `base.html`, the home menu item, the
newest page plus the first page using each directive, and search with and
without a tag — each anonymous and logged in; markdown goes through
`markdown::render_checked`, which reports the partials it rendered and their
failures (normally logged and replaced by an inline error). Then every
template renders with each of `TEMPLATES`' example contexts (both login
states; search with and without a tag/query, pagination), covering branches the
real data does not reach. Returns a `SmokeReport` (`cases`, `errors`:
deduplicated `SmokeError { template, line, message, case }`); `Err` when one of
its own queries fails (a tag deleted mid-run just skips its search case) — the helpers it shares with live requests treat DB
errors as live requests do (empty menu, no tags, directive "not found").
Publish (#115) runs it over the draft view before anything goes live (see [publishing](#design-overrides), step 2); `design_render_check` (#118) comes later.

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
file_blobs          hash PK, data bytea? (only STORAGE_KIND=db, m_033),
                    size_bytes (deduped by hash; one row per blob on every backend)
storage_objects     key PK, data bytea, etag (sha256 of data), updated_at
                    -- keyed objects (design/…, design-draft/…,
                    -- design-history/…) of STORAGE_KIND=db only (m_034)
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
                    — note m_031 emptied this table and assistant_events for
                    the 0.6 upgrade (#97): assistant history is disposable and
                    `assistant_events.payload` is a serialized third-party
                    enum both readers hard-error on, so the bump purged rather
                    than shimmed, the same call m_023 made —
                    temperature?, reasoning_effort? (m_027), max_output_tokens?,
                    thinking_budget_tokens? (m_028) — session-level
                    `GenerationParams` overrides, #42; `None` leaves that knob
                    at the model's own default), agent_profile (m_027,
                    default `"build"` — the engine profile the session runs
                    under, `"build"`/`"researcher"`/`"page-writer"`),
                    parent_session_id? + root_engine_session_id (m_032, #99 —
                    a spawned researcher/page-writer sub-agent is a real row
                    of its own; parent_session_id is a self-FK ON DELETE
                    CASCADE, so deleting a root takes its whole sub-tree.
                    root_engine_session_id is the *engine* id the row's
                    assistant_events are filed under — its own on a root, the
                    root's on a child — deliberately not a pointer to the root
                    *row*: /compact repoints a root's engine_session_id to a
                    fresh successor while the pre-compaction log stays under
                    the old key, so a row pointer would resolve a child to the
                    successor's log and read back a blank transcript. Written
                    by ws_bridge/child_rows.rs and handlers/sessions/
                    subagent_links.rs),
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

## Storage

File and thumbnail bytes go through `Storage` (`src/storage/`, held as `AppState.storage`; the AI file tools, markdown text directives and the export bundle get a clone). It is content-addressed: `put_blob(bytes) -> sha256`, `get_blob(hash)`, `get_blob_stream(hash)` (the public `/files/{hash}` and `/files/{hash}/nahled` routes stream it).

| `STORAGE_KIND` | Bytes live in | Notes |
|---|---|---|
| `db` (default) | `file_blobs.data` | The original behavior; a metadata-only row gets its bytes filled on the next put |
| `fs` | `STORAGE_DIR/blobs/{hash[0..2]}/{hash}` | Atomic writes (temp file + rename), fsync |
| `s3` | `{bucket}/blobs/{hash[0..2]}/{hash}` | `object_store` AWS client; 5 s connect / 30 s read timeout, 2 retries within 15 s, no total timeout (long downloads stream) |

**Keyed objects** sit next to the blobs on every backend (`src/storage/objects.rs`): `put(key, bytes)` / `get(key)` / `delete(key)` (idempotent) / `list(prefix)` (recursive under the directory `prefix/`, sorted, each with a `Version` = ETag + size + mtime) — object keys on `fs`/`s3`, `storage_objects` rows on `db` (m_034; ETag = sha256 of the data, mtime = `updated_at`, written as one upsert). Keys are validated (`parse_key`: no empty/`.`/`..` segments, no control characters). `scoped(prefix)` puts every key under `prefix/` on every backend (a `PrefixStore` on `fs`/`s3`, a key prefix on `db` — `db` blobs stay shared `file_blobs` rows); tests isolate themselves this way. The design (`design/…`, `design-draft/…`, `design-history/…`) is stored this way.

Every backend keeps one `file_blobs` row per blob (hash, size) — the FK target of `files.hash`/`file_thumbnails.hash` and the list `storage migrate` walks; only `db` fills `data` (nullable since m_033). Writes put the object **before** the row, so a row never points at unwritten bytes; a DB failure after the put leaves a harmless orphan (content-addressed, and there is no blob GC). Thumbnails are best effort: a failed thumbnail write only means `has_thumbnail: false`.

Errors: an unreachable `fs`/`s3` backend is `storage::Error::Unavailable` → API **503** `storage unavailable` (writes change nothing) and public serving 503; on `db` a database failure is a plain DB error → **500** `internal error` (detail only in the log); a `files` row whose blob the backend lacks is 404 publicly; a markdown directive whose blob can't be read renders like a missing file (logged). Blob keys are validated as lowercase sha256 hex, so nothing can escape the `blobs/` prefix.

**Switching backends:** `site_cli storage migrate --from db | --from-dir <path>` copies every hash in `file_blobs` and every keyed object of the source (`list_all`: everything but `blobs/`) into the configured `STORAGE_KIND` — idempotent (a present blob is verified by sha256 and skipped), the source is only read, a target blob or object with different content is reported and kept (exit 1), every copy is read back and verified; the summary counts blobs and objects separately. Reads are strict afterwards: no fallback to the old backend. Run it, then switch `STORAGE_KIND`. Moving back to `db` works the same way (`--from-dir` with `STORAGE_KIND=db`); `m_033`'s `down` refuses while metadata-only rows exist.

## Design overrides

A deployment's own design lives in storage (`fs`: under `STORAGE_DIR/`, `s3`: the bucket, `db`: `storage_objects` rows), `path` always under `templates/`, `assets/` or `mdcast/`:

| Keys | What |
|---|---|
| `design/{path}` | The **published** design, what the public site serves over the baked bundle (`src/design/stored.rs`). Held in RAM (`DesignStore.stored`, swapped wholesale) — requests never touch storage |
| `design-draft/{path}` | The one shared **draft** per site (`src/design/draft.rs`), edited by any admin; invisible to the public site |
| `design-history/{id}/{path}` + `design-history/{id}/meta.json` | A full snapshot of the draft per publish (`src/design/publish.rs`), `id` = the UTC RFC 3339 publish time (`2026-10-10T12:00:00.123456Z`, sorts chronologically), `meta.json` = `{id, at, by (username), files}`. Never deleted |
| `design-draft.json` | The draft's meta (`DraftMeta`): present = initialized; its **base** = the sha256 of every `design/` object the draft was started from (a history id alone would miss bucket edits) |
| `design-publish-pending.json` | Set only while a publish is between its snapshot and its history entry (recovery, below) |

Both the published design and the draft are read **over the baked bundle**: a path neither holds shows its baked default. Deleting a baked file from the draft therefore *reverts* it to the default, and an initialized draft with no files at all is the pure baked bundle.

**Draft lifecycle.** Until its first mutation the draft is *uninitialized*: GETs show the published view, report no changes and **never write**. The first mutation (`draft_put`/`draft_delete`, `site_cli design push`, discard, restore) copies the published view (baked ∪ `design/` in storage) into `design-draft/` and writes the meta last (an interrupted init is redone), so the draft holds the full bundle. Discard and restore **re-base** it on the current `design/`.

**Draft API** (`DesignStore::{draft, draft_read, draft_put, draft_delete, draft_discard}`, serialized by the draft mutex, which also caches the draft's bytes by version): raw bytes in and out, not validated (the draft may be broken while being worked on). `draft()` returns `initialized`, its files and the **change list** — the draft view against the published view (RAM), `added`/`modified`/`deleted` per path. Every mutation broadcasts `design.draft_changed` on the WS hub.

**Publish** (`DesignStore::publish(db, storage, templates, by, force)`, under `reload_lock` then the draft mutex, so within the server process publishes, reloads and draft edits never interleave — `site_cli design push` is outside that, see "Ways in"):
1. complete a pending publish if the marker is set (below), reloading after it;
2. checks, each rejection changing nothing: an uninitialized draft → **409** `nothing to publish`; its full view (the draft over the baked bundle, exactly what goes live) fails validation (`check_view` in `publish.rs`): `stored::validate_for_publish` (every template UTF-8 + MiniJinja compile), then the strict [smoke render](#template-contract) over that view against the site's real data → **422** listing `templates/<name>[:<line>]: <message>` (smoke errors add `(rendering <case>)`); the smoke render failing to query the DB → **500** `the design render check failed` (detail only in the log); read the current `design/` from storage — if the draft view equals the live view → **409** `nothing to publish` (no version is created); if `design/` changed since the draft's base (bucket edit, a push to `design/` by hand, a publish from a stale process) → **409** `design/ changed outside the draft since it was started: <paths>` unless `force` (`POST /api/design/publish?force=true`), which overwrites them;
3. pick the version id (the current UTC time, bumped by 1 µs while `design-history/{id}/` already holds anything) and write the snapshot `design-history/{id}/…`;
4. write the pending marker (the history entry);
5. **mirror** draft → `design/` (write what differs, then delete what the draft lacks — keys under the bundle roots only);
6. reload (list, validate, swap the RAM overlay, recompile templates);
7. write `meta.json`, delete the marker, re-base the draft on the new `design/`.

Visitors switch designs only at step 6's RAM swap, so the running site never serves a half-mirrored `design/`.

Failure guarantees:
- **Rejected or storage down before step 4**: live `design/` and the running design untouched; at worst an orphan snapshot without `meta.json` (ignored by the history).
- **Mirror or reload fails (steps 5–6)**: the previous `design/` objects are mirrored back from step 2's copy (each path holds either the old or the new bytes, so the reverse mirror restores exactly) and only then the marker is dropped; the error (503 storage unavailable / 500) says exactly which happened — `previous design restored`; `previous design restored, but the publish marker could not be cleared`; or `restoring the previous design failed` — the latter two adding that the next reload, publish or start completes this publish. The running design never changed, the draft keeps the unpublished edits.
- **A marker left behind** (crash after step 4, or either failed cleanup above) is resolved as early as possible: at the start of **every** reload (including startup's) and publish, under `reload_lock` (and the draft mutex for its draft part, same lock order). Its snapshot — complete and validated before the marker was written — is rolled forward: mirrored to `design/`, recorded (`meta.json`), the marker dropped. The draft's base moves to the snapshot only if the draft's files still equal it; a draft changed since (discarded, restored, edited) keeps its base, so its next publish answers 409 for the rolled-forward paths instead of silently reverting them. A reload that completed a pending publish says so in its status (`last_reload.completed_publish`) and broadcasts `design.published`; one completed at the start of a publish is only logged. A marker whose snapshot file count does not match is dropped without touching `design/`. So a stale marker cannot sit around and later overwrite newer bucket edits.
- **Recording fails after going live (step 7)**: the publish is live and reported as such; a missing history entry is written by the next reload/publish/start (the marker stays), a stale draft base only makes the next publish ask for `force`.

**History / restore:** `DesignStore::history` lists every `meta.json`, newest first; `restore(id)` copies that snapshot into the **draft** (never straight to live) and re-bases it — publish it to go live.

**Baked updates are shadowed.** After the first publish `design/` holds a full copy of the bundle, so improved baked files shipped by a later release do **not** reach the site on their own — the deployment's design is pinned. To adopt one, compare it in the admin (`GET /api/design/draft/{path}?source=baked` vs `?source=published`) and revert that file in the draft (`DELETE /api/design/draft/{path}` — the draft view falls back to the new baked default), then publish. Discarding the draft does *not* adopt baked changes (it copies the published `design/` back).

**Reload** (`DesignStore::reload`, under `reload_lock`): complete a pending publish (above), list `design/`, download only objects whose version (ETag + size + mtime) changed, syntax-check every template, then swap the overlay in and recompile the release build's frozen template environment (`Templates::refresh`). A failure leaves the running design untouched. It records its outcome (`last_reload`, shown in the admin; storage and DB failures appear only as `storage unavailable` / `database error`, the detail goes to the log) either way.

**Ways in:** the admin Design page (edits the draft through `/api/design/draft/*`; interim "Draft — not live" label with Publish — confirming a forced publish on a conflict — and Discard buttons until the Design studio, #119); `site_cli design push <dir>` (uploads a folder in the bundle layout **into the draft**, initializing it if needed, skips other paths, keeps draft files the folder lacks, refuses the whole push on a broken template; it goes live through the same publish. The CLI is another process: it emits **no WS events** (open admin tabs see the push on their next refresh) and takes **no lock** shared with the server, so it must not run while an admin publishes, discards or restores — the draft could end up mixed); or, for professionals, edit `design/` objects directly in the bucket (Garage admin UI, `aws s3`, rclone) and click **Reload** (`POST /api/design/reload`). A bucket edit bypasses the draft: the next publish answers 409 listing the edited paths — **discard** the draft (after Reload) to adopt them, or force to overwrite them.

**Startup** runs a reload (completing a pending publish first) before serving; unreachable storage or a broken override refuses the start rather than serving the baked design in place of the site's own. Every backend holds the design (`db` since #114). Reloads and the draft cache are per process (single replica assumed). MCP and the AI assistant have no design access yet (#118).

### Draft preview

An admin sees the real site rendered with the draft (#116): `POST /api/design/preview {on: true}` sets the `design_preview` cookie, which counts **only together with a valid session** — `Look::resolve` (`src/routes/public/preview.rs`) checks the session only when the cookie is present (requests outside preview cost nothing extra), and an anonymous request or a bogus session gets the published design, cookie or not. In preview, public pages (catch-all, search), `/assets/*` and both exports (mdcast templates + `brand.toml`) resolve from the **draft view** — draft over baked (an uninitialized draft = the published view); `DESIGN_DIR` is ignored, since the preview shows what a publish would put live. Every preview response is `Cache-Control: no-store`; HTML gets a fixed "NÁHLED DRAFTU" banner with the exit link, injected right after `<body…>` (prepended when there is none), independent of the draft's templates. A template error renders a page listing each template in the error chain with name, line, message and the source excerpt (500, banner included) instead of the generic error page; a draft that cannot be loaded answers 503 with the non-leaking `status_error` text.

**Caching.** `DesignStore::draft_site` (`src/design/view.rs`) returns a `DraftSite` — the draft view's files plus its own MiniJinja environment (templates compile lazily and stay cached in it) — rebuilt only when its **stamp** changes: the published `Stored` (pointer identity; swapped wholesale by every reload/publish) and the draft's `(path, Version)` list (`None` while uninitialized). Every preview request re-reads the draft meta and re-lists `design-draft/` under the draft mutex (downloading only objects whose version moved, like a reload), so an edit from any surface — admin API, `site_cli design push`, a bucket edit — shows on the next request; unchanged, the cached site is reused. `Resolve` (`load` + `list_prefix`) is the seam: `DesignStore` (live) and `DraftSite` both implement it, and the template engine (`templates::environment`) and the export bundle take it.

## Routes

### Public (server-rendered)

| Path | Method | Description |
|---|---|---|
| `/files/{hash}` | GET | Full file (content-addressed, cacheable) |
| `/files/{hash}/nahled` | GET | Thumbnail |
| `/tag/{id}` | GET | 302 redirect to `/search?tag=<name>` |
| `/search?q=...` | GET | Fulltext search |
| `/sitemap.xml` | GET | Sitemap |
| `/assets/{*path}` | GET | Static files (`DESIGN_DIR` override → storage `design/assets/…` → baked `design/assets/{css,js,img}`; the draft's in [preview](#draft-preview)). `Vary: Cookie`, so toggling preview misses the day-long browser cache |
| `/{*path}` | GET | Catch-all: menu → page → 404 |
| `/{*path}?format=pdf\|slides` | GET | Export the resolved menu/page to PDF or reveal.js slides (see [Export (mdcast)](#export-mdcast)) — no `format` param passes straight through to the catch-all above |

### Admin SPA

| Path | Method | Description |
|---|---|---|
| `/admin` | GET | SPA entry (`index.html`) |
| `/admin/{*path}` | GET | Static from `client/dist/` via `rust-embed`; SPA fallback to `index.html` |

### JSON API `/api/*` (session cookie required)

`auth/{login,logout,me}`, `users` (`GET/POST /`, `DELETE /{id}`, `PUT /{id}/password`), `pages` CRUD + `paths` + revision restore, `tags` CRUD, `files` CRUD (multipart upload, 50 MB), `galleries` CRUD + `paths`, `menu` CRUD, `tokens` (list/create/delete), `markdown/render`, `paths/children`, `export/pages/{id}` (below), and everything under `assistant/*`: `sessions` CRUD + `sessions/{id}/messages` + `sessions/{id}/messages/{message_id}/approve` + `sessions/{id}/compact` (manual context compaction, #40), `mcp-servers` CRUD, `providers` CRUD + `providers/status` (live per-provider throttle posture, #89), `models` CRUD (`context_window` field, #40), `permissions` CRUD (tool-permission rules).

| Path | Method | Description |
|---|---|---|
| `/api/ws` | GET (upgrade) | Global authenticated WebSocket — see below |
| `/api/export/pages/{id}?format=pdf\|slides` | GET | Export any page by id to PDF or reveal.js slides (see [Export (mdcast)](#export-mdcast)) |
| `/api/design/draft` | GET | Draft state (read-only; an uninitialized draft shows the published view): storage kind, `local_dir`, `last_reload`, `initialized`, the draft view's files (`path`/`baked`/`overridden` = differs from baked/`size`) and `changes` vs published (`{path, kind: added\|modified\|deleted}`) |
| `/api/design/draft/{*path}` | GET / PUT / DELETE | Draft file as raw bytes (`?source=draft` default \| `published` \| `baked`) / write it from the raw body (20 MB, text or binary, not validated) / remove the draft's copy (a baked file reverts to its default) — writes answer the draft state and broadcast `design.draft_changed`; 400 bad path, 404 not in the draft |
| `/api/design/draft/discard` | POST | Reset the draft to the published view and re-base it; draft state, `design.draft_changed` |
| `/api/design/publish[?force=true]` | POST | Publish the draft (see [Design overrides](#design-overrides)) → the new history entry, `design.published`. Rejections change nothing: 422 failed validation (compile or strict smoke render, listing template + line), 500 when the smoke render cannot query the DB, 409 nothing to publish, 409 `design/` changed outside the draft (lists the paths; `force=true` overwrites them). 503 storage down / 500 otherwise when the mirror or reload failed (the message says whether the previous design was restored) |
| `/api/design/history` | GET | Every published version `{id, at, by, files}`, newest first |
| `/api/design/history/{id}/restore` | POST | Copy version `id` into the draft (never live) and re-base it; draft state, `design.draft_changed`; 404 unknown version |
| `/api/design/preview` | POST | `{on: bool}` → sets (`design_preview=1`, HttpOnly, SameSite=Lax, Path=/) or clears the preview cookie; echoes `{on}`. See [Draft preview](#draft-preview) |
| `/api/design/preview/exit` | GET | The preview banner's exit link: clears the cookie, 303 back to the `Referer`'s path (`/` without one) |
| `/api/design/reload` | POST | Complete a pending publish if any (then `last_reload.completed_publish` names it and `design.published` is broadcast), reload `design/` from storage (after edits made directly in the bucket) → draft state; 422 broken template, 503 storage down (fs/s3; db errors are 500) |

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
- **Files:** `file_list`, `file_create` (mimetype inferred from the path extension when omitted — `.pgn`/`.mmd`/`.fen`/`.json` get dedicated mimetypes, issue #57 — and the response's `embed` field is a ready-to-use directive derived from that extension/mimetype: `<pgn>`/`<mermaid>`/`<fen>`/`<json>` for those extensions, `<image>` for `image/*`, `<file>` otherwise — `files_repo::embed_hint`, issue #55), `file_read` (`include_content` returns the file's text for text-ish mimetypes — plain text, JSON, PGN, mermaid, FEN, per `files_repo::is_text_content`), `file_update` (path/description, plus optional `mimetype`/`data`/`data_base64` to replace the stored bytes in place — issue #56, so a bad upload is repairable instead of unrecoverable), `file_delete`
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

A fenced code block with info string `mermaid` also renders as a diagram. **Single source of truth:** the human/AI-facing description is the `MARKDOWN_EXTENSIONS_DOC` const (`src/markdown/mod.rs`), reused verbatim by the MCP server instructions, the AI system prompt, and the local `site_tools` description — edit it there, not in each surface. Each directive renders through a `templates/markdown/*.html` partial with a typed context (`*Partial` in `src/templates/context.rs`); a new directive or partial also needs an entry in `templates::contract::TEMPLATES` and `make contract`.

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
  actual pinned model — which is why that default must resolve
  deterministically (see `catalog.rs` below). Three submodules (kept under the 400-line cap):
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
    #43 against entanglement 0.6.0's cascading `resume` (ADR-0112, which
    re-materializes a root's whole spawn sub-tree and re-announces each
    child's `SessionStarted` exactly like a live spawn): this cache's resume
    population needs no extra code — the same generic watcher already covers
    a cascaded child — but the retry race is *not* eliminated, only widened to
    also cover a resume-reconstituted child's re-announced `SessionStarted`,
    not just a freshly live-spawned one. Neither `SESSION_PARENTS` nor the
    retry is a library-guaranteed problem this site can drop.
- `catalog.rs` — `SiteCatalog`: builds `entanglement_provider::LlmFactory`/
  `ModelResolver` closures from `llm_providers`/`llm_models`; `refresh()` is
  called after provider/model CRUD. The per-provider-`kind` factory dispatch
  itself lives in `src/ai/catalog/factory.rs` (`build_factory`,
  `ollama_base_url`, the `positive_u32`/`positive_usize` clamps), split out to
  keep `catalog.rs` under the 400-line cap. `refresh()` reads `llm_models`
  **`ORDER BY id`** and picks the engine-wide default through
  `choose_default_model_id`: the lowest-id row flagged `is_default`, else the
  lowest id overall. `llm_models` has no single-default constraint and
  Postgres' physical row order is not stable (an `UPDATE` relocates a row), so
  without that both the flagged scan and the unflagged fallback would re-decide
  the default on every restart — and with it `EngineConfig.context_window` for
  every fresh session. `ModelResolver` populates
  `ResolvedModel::context_window` from each model row's own `context_window`
  (#40), so a live `SetModel`/session resume budgets the turn loop's
  overflow handling against the model's real window instead of the engine's
  generic fallback (`entanglement_core::context::CONTEXT_LIMIT_TOKENS`).
  `build_factory` also threads each row's `concurrency`/`rpm` (m_026) into the
  `*_factory` calls (ADR-0111 in `entanglement_provider`, #41): every session
  built from that row shares its provider's own per-endpoint `HttpClient`
  state keyed by (base URL, api key), so `concurrency` caps simultaneously
  in-flight requests (held across the whole streamed turn — the storm guard
  for many spawned sub-agents) and `rpm` sets the adaptive pacing gate; a 429
  backs the gate off and a success relaxes it. Both are nullable — `None`
  falls back to the library's own client defaults (3 concurrent / 50 rpm).
  The values are also exposed on `CatalogModel.concurrency`/`.rpm` for
  introspection, even though they're already baked into the `llm_factory`
  closure. `refresh()` builds a **fresh `HttpClient` per provider row** every
  call — `src/ai/catalog/throttle.rs`'s `by_provider_id: HashMap<i32,
  ProviderHandle>` (split out to keep `catalog.rs` under the 400-line cap) —
  rather than reusing one long-lived instance shared across every provider:
  `entanglement_provider`'s `HttpClient` locks in an endpoint's rpm/concurrency
  on that endpoint's *first* request and ignores later values for the same
  key, so a stale client would make an admin's `concurrency`/`rpm` edit
  silently have no effect once any turn had already gone through that
  provider. Splitting the client per provider (instead of one shared across
  all of them) is also what makes `SiteCatalog::throttle_statuses()` (#89)
  addressable *per provider* — `HttpClient::throttle_status()` only ever
  reports the single most-throttled endpoint on whatever pool it owns, with no
  way to attribute it to a `provider_id` if several providers shared one
  client. `GET /api/assistant/providers/status` polls `throttle_statuses()` to
  show the admin providers view each provider's live posture (idle / in-flight
  count vs. cap / backing-off countdown / pacing-penalized) instead of
  guessing why the assistant is slow. `generation_resolver()` builds the
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
  `CONNECT_TIMEOUT`, listing its tools by a 10s `LIST_TOOLS_TIMEOUT` (issue
  #28); both live in the `mcp/transport.rs` submodule alongside the pool
  client. Since entanglement 0.6 (#97) the streamable-
  HTTP transport lives in `entanglement-provider::mcp` (ADR-0153) and
  `connect` takes the provider's own `HttpClient`, so MCP calls ride the same
  per-endpoint pool as LLM traffic — retry schedule, concurrency cap
  and 429/`Retry-After` cool-down (ADR-0157). The pool's *request pacing* is
  deliberately disabled for MCP: `transport::mcp_pool_client` builds it with
  `RetryConfig { rpm: MCP_ENDPOINT_RPM, ..default }`, an unreachable 60 000
  rpm, because the default 50 rpm (1.2s between requests to one endpoint,
  ~2.4s to reach `tools/list`, per server, sequentially, per cache refresh) is
  calibrated for a rate-limited LLM API, not for a user's own servers — LLM
  traffic in `catalog.rs` keeps the defaults. Because each endpoint's pool
  state is a `.state`/`.lock` pair on disk that nothing else evicts,
  `state.rs::create_state` runs a best-effort `prune_stale` sweep
  (`prune_endpoint_state`) at startup — idle >1h and provably dead pairs only,
  logged, never fatal. Holds the same `SharedRegistry` handle
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
  entanglement 0.6.0's `resume` cascades over the *whole* spawn sub-tree
  itself (ADR-0112), re-materializing a child that was still live as of where
  the log stopped, so no per-child loop is needed here. That one shared
  `root_session_id` is exactly why `assistant_sessions.root_engine_session_id`
  (m_032, #99) stores an *engine* id rather than a pointer to the root row:
  every one of these three filters (`resume_session`,
  `handlers/sessions/turn::load_prior_records`, `delete_session_events`) keys
  on the engine id, and a `/compact` moves the root row's
  `engine_session_id` to a fresh successor while the pre-compaction log stays
  under the old key — so a child holding a row pointer would resolve to the
  successor's (empty-of-it) log and read back a blank transcript even though
  its own records are fully intact.
  `handlers/sessions/turn/collect.rs`'s `send_and_collect` builds its own response
  from the `LogRecord`s it just observed rather than re-reading
  `assistant_events` after a turn settles — reassessed for #43 and unrelated
  to `entanglement_runtime`'s own guarantees either way: it exists solely
  because `DbSink`'s async writer task gives no read-your-writes guarantee at
  the instant this handler observes e.g. `Done`.
- `projection/` — pure fold of **one** session's `assistant_events` rows into
  the `{role, content}` shape the admin client renders (`role` one of
  `user | assistant | tool_result | error`). `assistant_events` for a whole
  session tree holds every descendant's interleaved rows under one
  `root_session_id`, so `project(records, target)` takes the session to fold
  explicitly (#100) — the callers pass the row's own `engine_session_id`
  (`read`, `send_message`/`approve`) or, after a compaction, the **successor**
  (`compact`). An unknown or empty `target` projects an empty transcript, not
  a panic.
  Every *other* session in the log becomes a **reference card**
  `content.sub_agents: [{agent_id, profile, task, message_count, preview}]` on
  the assistant message whose `tool_calls` include the call that produced it —
  no nested `messages`: since #99 a sub-agent is an `assistant_sessions` row
  of its own, so its transcript is read by opening that row, and
  `handlers/sessions/subagent_links.rs` splices the `child_db_session_id` to
  open onto each card after hydration (keeping the fold itself DB-free and
  unit-testable). `preview` is the child's last assistant text, truncated, so
  the parent transcript still reads without a click. This retires the old
  `role: "sub_agents"` leftover message, which was the invisible-grandchild
  bug: a grandchild matched no call in the root's own fold and landed in a
  bucket `AssistantMessageContent.vue` has no branch for, rendering as
  nothing. It now cards normally on its own parent's transcript. The match is
  *structural*,
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
  entanglement-core 0.6.0 for #43/#87/#97 and still true (0.6 added a
  multimodal `content: Vec<ContentPart>` to it, but no error flag). Since #98
  the fold also keeps model thinking: `OutEvent::ReasoningDelta` (always
  persisted — the runtime's tap has no allowlist) accumulates into an optional
  `content.reasoning` string on the enclosing assistant message, emitted *only*
  when non-empty so a turn without thinking stays byte-identical to before.
  Because `OpenTurn::flush_into` already fires on every `ToolOutput`, a
  multi-round turn attributes each round's reasoning to that round's own
  assistant message rather than piling it all onto the last one. This is
  **display-only and never replayed to a provider**: `entanglement-provider`'s
  Anthropic SSE reader discards `signature_delta` (`anthropic/sse.rs:192-194`),
  so a thinking block couldn't be replayed verifiably anyway, and core's own
  context rebuild drops `ReasoningDelta` outright (`session/replay.rs:132-134`).
  `turn.rs` holds `OpenTurn`/`flush_into`/`mark_resolved_calls`, split out of
  `mod.rs` for the 400-line cap.
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
  way out, plus `compact.rs`'s `sessions/{id}/compact`, #40,
  `subagent_links.rs`'s `hydrate_child_rows` and `tree.rs`'s root/child
  resolution — see below), `mcp_servers.rs`, `providers.rs` (CRUD +
  `providers/status`, live per-provider throttle posture from
  `SiteCatalog::throttle_statuses()`, #89), `models.rs` (`context_window`
  field, #40), `permissions.rs`.
  - **Reading a session (#100):** `GET /sessions/{id}` resumes and loads the
    *tree's* log by the row's `root_engine_session_id`, then projects the
    row's own `engine_session_id` out of it — identical for a root row (the
    two columns match), and what makes a sub-agent row return its own
    transcript at top level instead of an empty one. Keying the load on a
    child's own uuid would both read zero rows and resume a blank engine
    session under that id.
  - **Root-only operations and interactive children (`sessions/tree.rs`,
    #101):** `tree::root_engine_id` (a plain `root_engine_session_id` read)
    is the log key for *every* `{id}`-taking handler, and
    `tree::require_root` refuses `PATCH`, `compact` and `DELETE` on a
    sub-agent row with a **409** — a child can't take a model/profile switch,
    a compaction successor is root-shaped, and deleting only the child's row
    strands events filed under the root. `DELETE` on a root additionally
    `CloseSession`s/`forget_live`s every descendant (`tree::descendants`)
    before the self-FK cascade removes their rows, and purges
    `assistant_events` for every log key the tree ever used (a compaction
    leaves pre-compaction children on the older key). `GET` and
    `POST .../messages`/`approve` *are* allowed on a child: upstream never
    closes a finished sub-agent, so `ensure_target_live` (`sessions/turn.rs`)
    resumes the **root** (whose ADR-0112 cascade re-materializes descendants),
    checks `SiteEngine::await_live` for the child, and only then rebuilds it
    from its own slice of the root's log via
    `persistence::resume_child_session`. `load_prior_records` stays
    root-keyed throughout, since approval routing scans every session's
    records. `GET /sessions` stays a flat array but is emitted in tree
    pre-order (`tree::order_for_tree`: roots by `updated_at` descending, each
    followed by its own children by `created_at` ascending) — a child row is
    created mid-turn, so a naive flat `updated_at DESC` sorts it above its
    own parent.
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
    only `engine_session_id` (and, since #99, `root_engine_session_id` with
    it — everything the row reads from here on, including any sub-agent it
    spawns next, lives under the successor) repoints to the successor, so `GET
    /sessions/{id}` (and every other DB-id-keyed handler) transparently
    follows the fork. The source's own `assistant_events` log is left
    intact but unreachable from the DB row (ADR-0101: "the original stays
    idle, intact, independently resumable") — sub-agent rows spawned *before*
    the compaction deliberately keep pointing at it, which is what keeps their
    transcripts readable. Broadcasts a `compacted` event
    over the `assistant` WS topic (see below) so another open tab on the
    session notices and refetches.
- `ws_bridge/` — a single process-wide task subscribing to
  `agent_engine.holly.subscribe()` (issue #16): forwards the engine's
  content/lifecycle `OutEvent`s (`Status`, `TextDelta`, `ReasoningDelta`,
  `ToolCallDelta`, `ToolCall`, `ToolRequest`, `ToolOutput`, `Done`, `Error`,
  `SessionHibernated`, plus — #17 — a sub-agent child's own `SessionStarted`,
  — #42 — `ModelChanged`/`GenerationChanged`/`AgentChanged`, so a live
  `/model`/generation/profile switch made from *another* tab is visible
  without a manual reload, and — #88 — `AmbiguousRetry` (ADR-0118), so an
  ollama "stream died" stop with no tool calls surfaces as a "retrying…" chip
  instead of a stalled turn) to `WsHub` as `assistant.*` envelopes, resolving
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
  - **Sub-agent session rows (`ws_bridge/child_rows.rs`, #99):** a child's own
    `SessionStarted` is the only event naming both its parent and its profile,
    so it is also where the child's `assistant_sessions` row is written —
    before the event is forwarded, so the client never sees a child it can't
    open. `ensure_child_row` derives `user_id`/`provider`/`model`/`model_id`/
    `root_engine_session_id` from the **parent row** (never
    `engine::user_id_from_session`, whose `SESSION_PARENTS` entry is evicted
    the moment the child hibernates or ends) and refuses outright when that
    row is absent rather than guessing an owner. Three writers race on the
    same child — this task plus `handlers/sessions/subagent_links.rs`'s
    `hydrate_child_rows`, called from `read`/`send_message`/`approve`/`compact`
    — so the insert is `ON CONFLICT (engine_session_id) DO NOTHING` (the m_023
    unique index) plus a `SELECT`, never check-then-insert.
    `hydrate_child_rows` rebuilds the same rows from the log a handler is
    already holding, walking it in order (topological for free: a grandchild's
    `SessionStarted` always follows its parent's). That closes the window
    where the REST response for the very turn that spawned a child would carry
    a null child id, and repairs anything the live task lost to a
    `RecvError::Lagged` batch, which never comes back on the broadcast.
    `db_session_id` keeps naming the **root's** row for a child's events
    regardless, since that is what the client keys the inline
    running-sub-agent card on.

**Admin UI — the session tree (`client/`, #103).** Because a sub-agent is a
session row of its own (#99), `GET /sessions` returns a *flat* list mixing roots
and children, which `client/src/composables/useSessionTree.ts` folds into the
spawn tree the sidebar renders: roots `updated_at DESC`, children `created_at
ASC` within a parent, arbitrary depth (a sub-agent can spawn its own), and every
input row placed exactly once — a row whose `parent_session_id` names a session
missing from the list (an **orphan**) renders at root level rather than
vanishing, and the same fallback breaks a corrupt parent cycle.
`AssistantSessionTree.vue` renders it collapsed by default (entanglement's
per-root spawn budget is 16, so a research-heavy chat contributes dozens of
child rows), auto-expanding the ancestors of whatever session is open. Delete is
offered on roots only — a child delete is refused server-side, and a root's own
delete cascades the sub-tree (m_032's self-FK), which is why
`stores/assistant.ts`'s `deleteSession` refetches the list instead of filtering
one id out of it. Entry points into a child are the tree itself and the
`content.sub_agents` card in the parent transcript, which
`AssistantMessageContent.vue` makes clickable when the card carries a
`child_db_session_id` (absent when the server never placed the child's row — the
card then renders inert rather than as a dead link); either way the child's own
transcript comes from `GET /sessions/{child_id}` at top level, and while it is
still streaming `AssistantView.vue` matches its live card on the envelope's
`child_db_session_id` as well as the root's `db_session_id` (#102) so the
child's view streams rather than sitting blank until the turn settles. On a child
session `AssistantSessionToolbar.vue` drops the model picker, the profile switch
and Compact, since those are fixed by the spawn — sending a message to a child
stays allowed.

Configured per user via the admin SPA: `/admin/{providers,models,assistant,mcp-servers,tool-permissions}`. Provider API keys live in `llm_providers.api_key` (set through the UI, never in `.env`).

## Export (mdcast)

Epic #63 integrated [`mdcast`](https://github.com/xmiksay/mdcast) to render pages to PDF and reveal.js slide decks; since mdcast **0.4** the site no longer compiles the engine (typst in-process + a pandoc subprocess) and instead speaks HTTP to a remote **`mdcast-server`** through the thin [`mdcast-client`](https://crates.io/crates/mdcast-client) crate (`Cargo.toml`: `mdcast-client` + `mdcast-api` with its `toml` feature; a renamed `reqwest12` dep exists only because `mdcast-client` pins reqwest 0.12 and the injected client is the one place timeouts can be configured). The server runs the whole mdcast pipeline — frontmatter extraction, page splitting, auto-classification against the request's `BrandSpec::auto_layout`, and the typst/pandoc backends — so the site sends raw multi-page markdown plus brand config and assets, and gets artifact bytes back.

**Configuration and availability:** `MDCAST_URL` names the server; unset means `AppState.mdcast: Option<mdcast_client::Client>` is `None` and both export routes answer `503` up front — nothing else in the site degrades. `MDCAST_TOKEN` is the bearer token; unset substitutes the literal placeholder `unauthenticated` at client construction (`export::build_client`), which a tokenless server ignores and a token-gated one rejects with a clear 401. `state::create_state` probes `GET /v1/capabilities` once at startup, **log-only** (targets/version on success, a warning on failure): a server that is down at boot starts working the moment it comes up, since every request is attempted regardless. Per-request failures map through `export::ExportError`: transport errors and 502-504 → `Unavailable` → HTTP 503, everything else → `Failed` → HTTP 500 with the chain logged.

**Asset negotiation (`src/export/bundle.rs`):** the wire protocol is content-addressed — a render request carries an `AssetManifest` (asset key → sha256), the server answers `409` naming only the digests its cache lacks, the client uploads exactly those and retries once. `build_bundle` declares everything a render references, in three layers:

- **bridge SVGs** (eager bytes) — `BridgedMarkdown.assets`, the synthesized fen/pgn/mermaid diagrams under `bridge/{fen,pgn,mermaid}/…`; they exist only in memory, so the bundle holds their bytes (upload still only happens on a cache miss).
- **design templates** (eager bytes) — every `design/mdcast/*` file except `brand.toml` (via `DesignStore::list_prefix`, so a `DESIGN_DIR` override applies exactly like it does to public pages), keyed with the `mdcast/` prefix stripped (`typst/layouts/pdf/hero.typ`, `revealjs/brand.css`). Manifest keys **shadow** the server's embedded catalog per mdcast 0.4's semantics, so these override the stock layouts per key while any key the site doesn't ship falls back to the server's default. Editing a template under `DESIGN_DIR` changes its digest and re-uploads automatically.
- **page images** (digest-only, lazy) — image destinations parsed out of the bridged markdown (plus `BrandSpec::logo` if ever set), resolved via `markdown::lookup::fetch_file`; `files.hash` *is* the manifest digest, so blob bytes are read from storage only inside the `409` path, when the server actually asks. An image with no `files` row is skipped with a debug log — the server warn+drops undeclared refs, the same soft failure the in-process pipeline had. Operator note: keep the server's `MDCAST_MAX_UPLOAD_BYTES` at or above this site's 50 MB file-upload cap, or large images surface as `PayloadTooLarge` → 500.

**The directive pre-render bridge (#66):** `mdcast`'s `PageSplitter::split` (and its typst backend) takes real markdown, not HTML, and typst has no notion of a raw HTML block/table — so a page's `<page>`/`<file>`/`<image>`/`<gallery>`/`<fen>`/`<pgn>`/`<mermaid>`/`<json>` directives can't be handed the same HTML `render()` produces for the browser. `markdown::render_for_export(md, db, storage, tmpl, logged_in) -> BridgedMarkdown` (`src/markdown/mod.rs`) resolves every directive to plain markdown instead: `<fen>`/`<pgn>` render a static chess diagram via the [`chess-diagram`](https://crates.io/crates/chess-diagram) crate (`chess_diagram::render_svg`/`chess_diagram::pgn::board_at` + `SvgRenderer`) and `<mermaid>` reuses the existing `mermaid-svg` render — none of these three have a `file_blobs` row of their own, so each synthesized SVG is spliced in as a `![alt](bridge/{fen,pgn,mermaid}/<hash>[-<ply>].svg)` markdown image reference and collected into `BridgedMarkdown.assets: Vec<(String, Bytes)>`; `<json>` renders a real markdown pipe table (`handlers::json::markdown_table`) instead of an HTML `<table>`; `<page>` splices the nested page's own directive-expanded markdown inline (trimmed, blank-line-separated) instead of wrapping it in a `page.html` template; `<file>`/`<image>`/`<gallery>` emit plain `![alt](file.path)` / `[desc](file.path)` references (by path — `build_bundle` re-resolves those paths through `files` to declare their digests). `render_for_export` deliberately stops after `renderer::expand_directives` — it never runs the pulldown-cmark parse, syntect highlighting, or `links::rewrite_internal_links` that `render()` does afterward, since all three operate on/produce HTML; plain non-directive markdown passes through completely untouched. `build_bundle` (above) carries `bridged.assets` into the render request eagerly, so the server's typst/pandoc backends resolve a synthesized diagram key exactly like any other asset. **Known limitation, explicitly out of scope for #66:** the alternate ` ```fen `/` ```pgn ` fenced-code-block authoring form (a second, independent path into the same chessboard.js client dependency, handled in `src/markdown/highlight.rs`) is *not* resolved by this bridge — only the `<fen>`/`<pgn>` directive-tag form is — so a page using the fenced form still exports as a plain, unrendered code block.

**The render pipeline + HTTP routes (#67):** `src/export/render.rs` exposes `ExportFormat` (`Pdf` | `Slides`, `::parse`/`::target`/`::content_type`) and the entrypoint `render_page(client, db, design, tmpl, markdown_src, title, logged_in, format) -> Result<mdcast_client::Artifact, ExportError>`: it runs `markdown::render_for_export`, loads the `BrandSpec` (see below), assembles the bundle via `build_bundle`, and posts a `RenderMarkdownRequest` (raw bridged markdown, `meta.title`, `brand`, target) through `mdcast_client::Client::render`, which handles the `409 → upload → retry` negotiation internally. Splitting and auto-classification happen **server-side** against the request's `brand.auto_layout`. One behavior delta vs. the old in-process pipeline: the server extracts a leading YAML frontmatter block from the markdown, and its `title` beats the request's `meta.title` (the page title passed here).

**BrandSpec / design integration (#68):** `render_page` sources `mdcast_api::BrandSpec` from `design/mdcast/brand.toml` (`render::load_brand`, via the request's design view — `Resolve`: the live `DesignStore`, or the draft in [preview](#draft-preview) — not the asset bundle, since `BrandSpec` travels as `request.brand`, caller-owned config the server hands to its splitter and backends, not something a backend requests mid-render) instead of `BrandSpec::default()`; a missing, non-UTF-8, or malformed file logs a warning and degrades to the default rather than failing the export. `design/assets/css/style.css`'s `:root` custom properties are the single source of truth the committed `brand.toml`'s `[palette]` mirrors (`background`/`text`/`heading`/`muted`/`accent`/`link`/`border` ← `--bg`/`--text`/`--black`/`--muted`/`--accent`/`--border`); `[fonts]` sets `sans`/`mono` for typst (constrained to typst-kit's bundled New Computer Modern/DejaVu Sans Mono — it ships no proportional sans face, so a real brand font is a `ResolvedDoc.fonts`-backed follow-up) and `body`/`heading`/`code` for reveal.js (a real browser, so it gets the site's actual font stack). Two more `design/mdcast/` additions consume it:

- `typst/layouts/pdf/{content,hero,callout,section-divider,thanks}.typ` — brand-aware overrides of mdcast's embedded layouts of the same name, reading `brand-color`/`brand-font` off the `/context.typ` accessors mdcast injects (`content.typ`'s header already used `doc-meta`; these also read `brand.palette`/`brand.fonts`). `image-full.typ` has no themeable text/color and is left on mdcast's embedded default. `typst/layouts/pdf-presentation/*` isn't overridden either — `Target::PdfPresentation` isn't wired to any route (only `Pdf` and `HtmlReveal` are, see `ExportFormat::target`).
- `revealjs/brand.css` — an escape hatch appended after mdcast's own `palette`/`fonts` → reveal.js CSS-custom-property projection (`reveal_brand::brand_css`, which also emits a `--brand-<key>` passthrough for every palette key); a few rules here (code-block border, blockquote accent bar, section-divider heading color) round out the "default reveal.js theme/shell" beyond what the CSS-variable projection alone covers.

Two routes call `render_page`, both refusing **every** format with `503` up front when `state.mdcast` is `None` (no `MDCAST_URL` configured — checked before any DB/render work), and mapping a per-request `ExportError::Unavailable` to `503` as well:

- **Public — `GET /{*path}?format=pdf|slides`** (`src/routes/public/export.rs`, registered as the wildcard `/{*path}`, so it's the effective fallback for every non-root path in the router). No `format` query param passes straight through to `public::catch_all` unchanged — plain page viewing is unaffected. With `format`, it reuses `public::lookup_content`/`PathContent` (`src/routes/public/mod.rs`, factored out of `catch_all` for exactly this reuse) for the identical menu → page → 404 lookup and `private && !logged_in` → 404 semantics `catch_all` already implements, then renders and returns the artifact with `Content-Type`/`Content-Disposition: attachment; filename="<slug>.<ext>"` (slug = the last path segment, sanitized by `export::sanitize_filename`).
- **Admin — `GET /api/export/pages/{id}?format=pdf|slides`** (`src/routes/api/export.rs`, nested under the `protected` router so `require_login_api` gates it): looks up the page by id, renders the same way, and returns the same headers. No privacy check beyond the login gate — any logged-in user can export any page, matching the rest of the admin API's permission model.

`export::sanitize_filename` (`src/export/mod.rs`) is shared by both routes: it maps every non-ASCII-alphanumeric/`-`/`_` character to `-`, so a page path can never smuggle a CR/LF or quote into the `Content-Disposition` header value.

## WebSocket Hub (`src/routes/ws.rs`)

`GET /api/ws` upgrades to a single per-tab WebSocket, authenticated the same way as the rest of `/api/*` (session cookie, checked before the upgrade). `WsHub` (in `AppState.ws_hub`) is a `DashMap<user_id, Vec<mpsc::Sender<Envelope>>>` registry; each open tab holds one entry.

Frames are JSON `Envelope { topic, event, payload }`, `topic` one of `assistant | pages | files | galleries | tags | design`:

- **`design`** — `draft_changed` after every mutation of the shared design draft (payload `{action: put|delete, path}`, `{action: discard}` or `{action: restore, version}`) and `published` (payload = the new history entry), broadcast to every connected user from the `src/routes/broadcast.rs` helpers (`design_draft_changed`/`design_published`).

- **`pages` / `files` / `galleries` / `tags`** — `created`/`updated` (payload = the same summary shape the REST endpoint returns) / `deleted` (payload `{ id }`). Broadcast to **every** connected user via `WsHub::broadcast`/`broadcast_serialized` — these are shared site entities, not per-user. Published from the shared `src/routes/broadcast.rs` helpers, called after a successful create/update/delete from all three mutating edges — `src/routes/api/{pages,files,galleries,tags}.rs`, `src/routes/mcp/{pages,tags,files,galleries}.rs`, and `src/ai/tools/*.rs` — so a mutation over MCP or by the AI assistant broadcasts the same event a REST API mutation would (#25).
- **`assistant`** — real, token-level streaming straight off `agent_engine.holly.subscribe()` (`src/ai/ws_bridge/`; the session-identity splice below lives in `ws_bridge/envelope.rs`), published only to the owning user's own connections via `WsHub::publish`. `event` is the forwarded `OutEvent`'s own `"kind"` tag and `payload` is that event's JSON shape (`entanglement_core::OutEvent` already derives `Serialize`) plus a spliced-in `db_session_id` (the `assistant_sessions.id` the engine's root `SessionId` resolves to, cached per session): `status` (`AgentState`: idle/thinking/waiting_approval/waiting_answer/done/error), `text_delta`/`reasoning_delta` (incremental text), `tool_call_delta` (incremental tool-input fragment), `tool_call` (display-only, full call), `tool_request` (needs approval — approve/reject the same way as an existing message's tool call, `POST .../messages/{any}/approve`, since the engine no longer keys approvals by message id), `tool_output`, `done`, `error`, `session_hibernated`, (#17) a sub-agent child's own `session_started` (`{session, profile, parent}`, `researcher`/`page-writer`), and (#88) `ambiguous_retry` (`{nudge}`, ADR-0118) — an ollama "stream died" stop with no tool calls and no confident finish signal, folded by the client's live-turn store into a transient `retrying: true` flag (cleared by the next `text_delta`/`done`/`error`) rather than any persisted transcript content, since `ai::projection::project` deliberately drops it (a round boundary, not a message). Any event belonging to a sub-agent child also carries `agent_session_id` (the child's own engine `SessionId`) so the client can render it nested under the spawning turn instead of the root's own top-level stream, plus `child_db_session_id` (#102) — the `assistant_sessions.id` of the child's own row, which since #99 it has. `db_session_id` deliberately still names the **root's** row for a child's events: the client filters the inline running-sub-agent card on it (`AssistantView.vue`'s `liveSubAgentsForCurrent`) and refetches the parent on the child's `done`, both of which would break if it meant the child — so the child's row id is strictly *additive*, which also removes any deploy-ordering constraint against the client. No `parent_db_session_id` is needed: `db_session_id` already *is* the root. Both child fields are absent on a root event, keeping that envelope byte-identical to pre-#17. `child_db_session_id` is resolved cache-only (seeded by `child_rows::ensure_child_row` when the child's `session_started` came through), so a child whose row never landed — its `session_started` lost to a broadcast lag, or its parent row not yet written — simply omits the field and keeps streaming under the root; `handlers::sessions::subagent_links` repairs the row out of band. `compacted` (#40) is the one `assistant.*` event *not* forwarded by `ws_bridge` — `handlers/sessions/compact.rs` publishes it directly once a manual compaction's fork/retire completes, carrying the real `OutEvent::Compacted` shape (`summary`, `kept`, `auto: false`) plus `db_session_id` and `successor_session_id`, so another open tab on the session notices its `engine_session_id` moved and refetches.

Server sends a WS ping every 30s; a failed send (or a client `Close` frame) drops that connection's sender from the registry. `WsHub::publish`/`broadcast` also prune any sender whose receiver has been dropped.

Client side: `client/src/stores/ws.ts` owns the single connection (reconnect with exponential backoff) and topic→handler dispatch; `client/src/composables/useListSync.ts` wires `pages`/`files`/`galleries`/`tags` events into the matching Pinia store's `items` list; `client/src/stores/assistantLiveTurns.ts` (composed into `stores/assistant.ts`) accumulates `assistant.*` deltas into a `live: LiveTurn | null` (the root turn) and, since #17, `liveSubAgents: Record<string, LiveSubAgentTurn>` keyed by `agent_session_id` — a sub-agent's events carry that field instead of belonging to the root, and it is tracked independently of `live` since a detached child keeps streaming after the root's own turn has already settled. Both are rendered by `AssistantView.vue` as in-progress bubbles (`LiveSubAgentTurn.vue`/`LiveToolCallList.vue`), and on a turn's own `done`/`error`/`session_hibernated`/`compacted` (#40) the matching entry clears and the session refetches over REST for the authoritative message list — for a settling child, that is the parent when the parent is the open session and (#102, off `child_db_session_id`) the child itself when *it* is the one open — reusing `ai::projection::project`'s per-session fold rather than re-implementing it in TypeScript — a fold whose `content.sub_agents` entries are, since #100, flat reference cards rather than nested transcripts, so `AssistantMessageContent.vue` is no longer self-recursive and instead renders each card as a link into that child's own session (#103, see "Admin UI" above). `client/src/App.vue` connects after login, disconnects on logout. `AssistantView.vue`'s header also has a "Compact" button (`assistant.compactSession`) that `POST`s `sessions/{id}/compact` and swaps in the returned successor-session detail directly.

## Docker

```bash
docker build -t site .
docker run -e DATABASE_URL=... -p 3000:3000 site
```

`docker-compose.yml`: `db` (Postgres 17-alpine, host port 5434) + `app` (gated behind `profiles: ["full"]`).

The runtime stage installs only `ca-certificates` (needed for TLS to the remote render server, among others) — since mdcast 0.4 the image contains no `pandoc` and no typst: PDF/slides export happens on a separate `mdcast-server` deployment reached via `MDCAST_URL`, see [Export (mdcast)](#export-mdcast).
