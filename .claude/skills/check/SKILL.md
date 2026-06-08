---
name: check
description: Run the build + test verification for the personal site before declaring a change done — lint (Rust fmt + clippy) and tests, in the order rust-embed requires. Wraps `make verify`.
---

# /check — verify the site

Verify the repo is green before any change is called done. Always drive this through the **Makefile** — do not retype raw cargo/npm commands.

## Steps

1. **Run the gate:**
   ```bash
   make verify
   ```
   `verify` = `lint` (`cargo fmt --check`, `cargo clippy --all-targets -D warnings`) + `test` (`test-unit`: `cargo test --lib --bins` · `test-integration`: guarded, no-ops until a `tests/` dir exists).
2. **If a build or local run is also requested**, use `make build` / `make run` — never bare `cargo build`/`cargo run`. The Vue admin SPA is embedded into `site_server` by **rust-embed** (`#[folder = "client/dist"]`), so `client/dist` must exist first; the `build`/`run` targets produce it before invoking cargo. A bare `cargo build` embeds a stale or missing SPA.

## Notes

- Unit tests live in `src/` (`#[cfg(test)]`, e.g. `templates.rs`, `design.rs`, `markdown.rs`); there are no integration tests yet — `make test-integration` no-ops until `tests/` exists. Add tests with new logic.
- The client has no standalone lint/typecheck script — `vue-tsc -b` runs as part of `make client` (the build). For admin UI changes, sanity-check with `make dev` or `make client`.
- `make run` needs `DATABASE_URL`; migrations auto-run on startup.
- Report pass/fail per stage with real output. Never claim green on red; fix clippy warnings (`-D warnings`) and re-run.
