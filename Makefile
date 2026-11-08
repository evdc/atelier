# Atelier dev tasks. `make help` lists them.
.PHONY: help dev server web app gen test check fmt

HOME_DEV ?= $(HOME)/.atelier-dev

help:            ## List tasks
	@grep -E '^[a-z]+:.*##' $(MAKEFILE_LIST) | sed 's/:.*## /\t/'

dev:             ## Run server (dev home) + web UI in a browser at http://localhost:5173
	cargo build -p atelier-server
	ATELIER_HOME=$(HOME_DEV) ./target/debug/atelierd & \
	ATELIER_HOME=$(HOME_DEV) pnpm -C web dev; kill %1

server:          ## Run only the server (dev home)
	ATELIER_HOME=$(HOME_DEV) cargo run -p atelier-server

app:             ## Run the desktop app (starts its own server in ~/.atelier if none is running)
	cargo build -p atelier-server
	cd app && cargo tauri dev

gen:             ## Regenerate TypeScript bindings from the Rust protocol types
	rm -rf web/src/gen && cargo test -q -p atelier-core -p atelier-proto export_bindings >/dev/null

test:            ## All tests
	cargo test --workspace
	pnpm -C web test

check:           ## Lint + typecheck everything (run before handing work back)
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	$(MAKE) gen && git diff --exit-code -- web/src/gen || (echo "web/src/gen is stale: commit the regenerated bindings"; exit 1)
	pnpm -C web check

fmt:             ## Format Rust
	cargo fmt --all
