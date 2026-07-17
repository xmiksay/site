# Testing

How tests are organized and how to add them. All test flows go through the
[`Makefile`](../Makefile).

## Running

```bash
make test            # everything: backend unit + integration + client
make test-unit       # Rust: cargo test --lib --bins
make test-client     # Vue: vitest (installs client deps if missing)
make test-integration # Rust tests/ — DB/Ollama-gated, skip gracefully if unset/unreachable (see below)
make verify          # pre-"done" gate: lint + all tests
```

> **rust-embed ordering:** `site_server` embeds `client/dist` via
> `#[folder = "client/dist"]`, so building the **binary** needs `client/dist` to
> exist first (`make build`/`run` enforce it). `make test-unit`
> (`cargo test --lib`) and `cargo check` do **not** — the library compiles
> without the embed — so unit tests run on a clean tree.

## Backend unit tests (Rust)

Pure-logic tests live **in-module** in a `#[cfg(test)] mod tests` block at the
bottom of the file under test — no `tests/` directory, no dev-dependencies.
Good targets are dependency-free functions; current examples:

- `src/path_util.rs` — `normalize` / `normalize_prefix` (slug canonicalization).
- `src/files.rs` — `hash_blob` (SHA-256 content addressing) against known vectors.
- `src/markdown/tests.rs` — the largest suite: directive parsing, tag allow-listing,
  container collection.

Pattern:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_and_collapses_slashes() {
        assert_eq!(normalize("/a//b/"), "a/b");
    }
}
```

Keep unit tests to **pure logic**. Anything that needs the database (SeaORM
queries, `put_blob`/`read_blob`, revision reconstruction) belongs in an
integration test, not here.

## Client tests (Vue / vitest)

Config: [`client/vitest.config.ts`](../client/vitest.config.ts) (jsdom env,
globals on). Specs are `client/src/**/*.spec.ts`.

Run:

```bash
cd client && npm run test        # once
cd client && npm run test:watch  # watch mode
# or from the repo root:
make test-client
```

The lowest-friction targets are **Pinia stores**. Mock the API layer, keep the
real `ApiError` so `instanceof` branches work, and reset Pinia per test. See
[`stores/auth.spec.ts`](../client/src/stores/auth.spec.ts) and
[`stores/tokens.spec.ts`](../client/src/stores/tokens.spec.ts) for the pattern:

```ts
import { setActivePinia, createPinia } from 'pinia'
import { api, apiVoid, ApiError } from '../api'

vi.mock('../api', async (importActual) => {
  const actual = await importActual<typeof import('../api')>()
  return { ...actual, api: vi.fn(), apiVoid: vi.fn() }
})

beforeEach(() => setActivePinia(createPinia()))
```

`@vue/test-utils` `mount()` is installed for component specs; see
[`views/LoginView.spec.ts`](../client/src/views/LoginView.spec.ts) for the
pattern (`mount()` + the same API/store mocking convention as the store
specs).

## Integration tests (`tests/`)

Real HTTP/DB integration tests live under `tests/` (`cargo test --test '*'`,
wrapped by `make test-integration`). Each test is gated on `DATABASE_URL`
being set — `eprintln!("skipping: DATABASE_URL not set"); return;` at the top,
not a `#[cfg]` — so `cargo test`/`make verify` stays green without a live test
DB, and the same binary runs for real against a scratch Postgres locally or in
CI:

```bash
docker run -d -e POSTGRES_DB=site_test -e POSTGRES_USER=site_test \
  -e POSTGRES_PASSWORD=site_test -p 5433:5432 postgres:17-alpine
DATABASE_URL=postgres://site_test:site_test@localhost:5433/site_test \
  cargo run --bin site_migration
DATABASE_URL=postgres://site_test:site_test@localhost:5433/site_test \
  cargo test --test '*'
```

`site_test` isn't reset between runs — every test creates its own throwaway
`users` row (a random tag/uuid in the username) and deletes it (cascading)
when done; see `tests/policy_db.rs`'s module doc for the full convention.

CI runs this suite for real: both `.github/workflows/backend.yml` (per-PR) and
`verify.yml` (post-merge) start a `postgres:17-alpine` service, export
`DATABASE_URL`, and run `site_migration` before the test step, so `tests/`
executes on every PR rather than self-skipping.

- `tests/policy_db.rs` — `SitePolicy`/`tool_permissions` resolution against a
  real `tool_permissions` table (FK to `users`, so it can't be faked
  in-memory).
- `tests/oauth_authorize.rs`, `tests/oauth_token.rs`, `tests/oauth_refresh.rs`
  — the OAuth2/PKCE flow (`src/routes/oauth/`) end to end over real HTTP:
  `GET`/`POST /oauth/authorize` param validation and the login form, the
  `authorization_code` and `refresh_token` grants at `POST /oauth/token`
  (PKCE verification, expiry, single-use codes, refresh rotation). Share
  fixtures/helpers via `tests/common/oauth.rs` (`#[path]`-included, not
  declared in `tests/common/mod.rs` — these two endpoints are form-encoded,
  not JSON, so they need their own request-building helpers).
- `tests/mcp_endpoint.rs`, `tests/mcp_pages.rs`,
  `tests/mcp_tags_files_galleries.rs` — the hand-rolled JSON-RPC `POST /mcp`
  server (`src/routes/mcp/`): Bearer-token auth, `initialize` (including the
  `CLAUDE`-page instructions override), `tools/list`, and `tools/call`
  dispatch/error shapes for every tool family. Shared Bearer/JSON-RPC helpers
  live in `tests/common/mcp.rs` (`#[path]`-included, same reason as
  `common/oauth.rs`).
- `tests/api_pages.rs`, `tests/api_files.rs`, `tests/api_galleries.rs`,
  `tests/api_tags.rs` — the session-cookie-protected `/api/*` REST layer
  (`src/routes/api/`) and the `src/repo/*.rs` logic it calls: page
  create/update/revision-restore/delete, file upload with SHA-256 dedup,
  gallery/tag CRUD and page↔tag association. Reuse `tests/common/mod.rs`'s
  `send()`/`test_db_url()` directly (session-cookie + JSON, same shape these
  need).
- `tests/tags_resolve.rs` — `site::repo::tags::resolve_ids` against a real
  `tags` table, using its own throwaway rows (not `tests/common/mod.rs`).
- `tests/ai_catalog.rs`, `tests/ai_persistence.rs`, `tests/ai_mcp.rs` — the
  `src/ai/` gaps: `SiteCatalog::load`/`refresh`/`model_resolver` against real
  `llm_providers`/`llm_models` rows; `DbSink` append/backpressure and
  `resume_session`'s integrity-gap refusal against `assistant_events`;
  `SiteMcp`'s per-user route cache/TTL and `known_tool_names` against
  `user_mcp_servers` (no live remote MCP server needed — failure-to-connect is
  itself part of what's exercised).
- The assistant-session flow — drives a session through the real HTTP API
  (`tower::ServiceExt::oneshot`, no socket) end to end: create, message, tool
  call, approve. Split by scenario across three top-level test files (each
  `tests/*.rs` compiles as its own binary, so this is the natural split
  boundary), sharing setup helpers via `tests/common/mod.rs` (`test_db_url`,
  `send`) and `tests/common/scripted.rs` (the scripted-`Llm` fixture, `#[path]`-
  included only where needed so the plain-Ollama test doesn't compile it):
  - `tests/assistant_session_base.rs` — the original acceptance test, using a
    real local Ollama model (`qwen3.5:9b`) — also gated on
    `http://localhost:11434` being reachable, skipped gracefully otherwise.
    Genuinely non-deterministic (a small model occasionally emits no tool
    call, or is slow/flaky under concurrent local load) — retried a few times
    in-test rather than asserted on the first attempt.
  - `tests/assistant_session_subagent_researcher.rs` and
    `tests/assistant_session_subagent_pagewriter.rs` — the sub-agent tests
    (#17), one file per sub-agent profile. These use a small scripted `Llm`
    instead (`ScriptedLlm` structs implementing `entanglement_core::Llm`,
    replying via `stream_from_response(LlmResponse { text, tool_calls })` —
    the same pattern `entanglement-core`'s own test suite uses) so a
    deterministic tool-call decision doesn't depend on a live model at all:
    DB-gated only, no Ollama gate, and dramatically faster (~1s vs 15-180s).
    `SiteEngine::spawn`'s last parameter, `llm_factory_override:
    Option<LlmFactory>`, exists solely as this seam (`None` in production,
    `Some(scripted)` in these tests) — see `ScriptedFixture`'s doc in
    `tests/common/scripted.rs` for why session creation must bypass `POST
    /assistant/sessions` (its `InMsg::SetModel` would rebind the session onto
    a real catalog-driven factory, discarding the override). **Prefer this
    pattern over a live model** for any new test that needs the engine to
    receive a specific tool call.
