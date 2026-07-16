# site — build/test/dev targets.
# IMPORTANT: the Vue admin SPA is embedded into site_server by rust-embed
# (`#[folder = "client/dist"]`), so client/dist must exist before cargo build —
# `build` and `run` enforce that ordering. (The `design/` bundle is also embedded
# but is source, not generated, so it needs no build step.)

export CARGO_BUILD_JOBS ?= 4

.DEFAULT_GOAL := help
.PHONY: help client build run migrate dev check fmt lint test test-unit test-integration test-client verify clean

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | awk 'BEGIN{FS=":.*?## "}{printf "  \033[36m%-18s\033[0m %s\n",$$1,$$2}'

client: ## Build the Vue admin SPA into client/dist (prereq for cargo build)
	cd client && npm ci && npm run build

build: client ## Build everything (client first, then the binaries)
	cargo build

run: client ## Run site_server on :3000 (embeds client/dist; needs DATABASE_URL)
	cargo run --bin site_server

migrate: ## Apply database migrations
	cargo run --bin site_migration

dev: ## Hot-reload the admin SPA (vite)
	cd client && npm run dev

check: ## Fast Rust typecheck
	cargo check --all-targets

fmt: ## Apply Rust formatting
	cargo fmt

lint: ## Rust fmt-check + clippy (client typecheck runs via `make client` / vue-tsc)
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

test-unit: ## Unit tests (in-module #[cfg(test)])
	cargo test --lib --bins

test-integration: ## Integration tests (tests/) — DB/Ollama-gated, skip gracefully if unset/unreachable
	@test -d tests && cargo test --test '*' || echo "no integration tests yet (tests/ absent)"

test-client: ## Vue admin SPA unit tests (vitest)
	cd client && { [ -d node_modules ] || npm ci; } && npm run test

test: test-unit test-integration test-client ## All tests (backend + client)

verify: lint test ## Pre-"done" gate: lint + tests

clean: ## Remove build artifacts
	cargo clean
	rm -rf client/dist
