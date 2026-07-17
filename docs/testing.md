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

`@vue/test-utils` `mount()` is installed for component specs, but none exist
yet — only the three store specs above. Adding the first component/view specs
is tracked as follow-up work.

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

- `tests/policy_db.rs` — `SitePolicy`/`tool_permissions` resolution against a
  real `tool_permissions` table (FK to `users`, so it can't be faked
  in-memory).
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
