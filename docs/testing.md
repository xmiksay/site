# Testing

How tests are organized and how to add them. All test flows go through the
[`Makefile`](../Makefile).

## Running

```bash
make test            # everything: backend unit + integration + client
make test-unit       # Rust: cargo test --lib --bins
make test-client     # Vue: vitest (installs client deps if missing)
make test-integration # Rust tests/ — currently a no-op (see below)
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
- `src/markdown.rs` — the largest suite: directive parsing, tag allow-listing,
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

Component specs use `@vue/test-utils` `mount()` (already installed).

## Integration tests (future — not yet present)

`make test-integration` gracefully no-ops while `tests/` is absent. Standing up
real HTTP/DB integration requires:

1. A live Postgres (the `db` service in [`docker-compose.yml`](../docker-compose.yml),
   or [`testcontainers`](https://docs.rs/testcontainers) spun up per test run).
2. Running the 22 migrations in `src/migration/` via the `Migrator` before the
   first query — the schema is Postgres-specific (raw SQL, `ON CONFLICT`,
   fulltext index), so there is no SQLite/in-memory shortcut.
3. Adding a `[dev-dependencies]` section to `Cargo.toml` (none exists today) for
   the test HTTP client / testcontainers.

Put such tests under `tests/` (e.g. `tests/api_pages.rs`); `make test-integration`
picks them up automatically once the directory exists.
