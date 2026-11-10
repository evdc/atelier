# Atelier dev tasks. `make help` lists them.
.PHONY: help dev server web app app-dev bundle ios-dev ios-build ios-run ios-device gen test check fmt

HOME_DEV ?= $(HOME)/.atelier-dev
# Dev servers get their own home and port, so they run beside the installed app's server (launchd, port 7747).
DEV_PORT ?= 7748
DEV_ENV = ATELIER_HOME=$(HOME_DEV) ATELIER_LISTEN=127.0.0.1:$(DEV_PORT) ATELIER_URL=ws://127.0.0.1:$(DEV_PORT)/ws

help:            ## List tasks
	@grep -E '^[a-z-]+:.*##' $(MAKEFILE_LIST) | sed 's/:.*## /\t/'

dev:             ## Run server (dev home) + web UI in a browser at http://localhost:5173
	cargo build -p atelier-server
	$(DEV_ENV) ./target/debug/atelierd & \
	$(DEV_ENV) pnpm -C web dev; kill %1

server:          ## Run only the server (dev home)
	$(DEV_ENV) cargo run -p atelier-server

app:             ## Run the desktop app over your real data (~/.atelier, port 7747), pausing the installed server
	cargo build -p atelier-server
	./scripts/app-real.sh

app-dev:         ## Run the desktop app against a fresh dev server (dev home, port $(DEV_PORT))
	cargo build -p atelier-server
	@# The app reuses a server already on its port; make sure that's the one just built.
	-@lsof -ti tcp:$(DEV_PORT) -sTCP:LISTEN | xargs kill 2>/dev/null
	cd app && $(DEV_ENV) cargo tauri dev

IOS_SIM ?= iPhone 17 Pro
IOS_APP := app/gen/apple/build/arm64-sim/Atelier.app

bundle:          ## Build Atelier.app with atelierd bundled as a sidecar (release)
	cd app && cargo tauri build --config bundle.conf.json

ios-dev:         ## Run the iOS app in a simulator ($(IOS_SIM)) against the Vite dev server
	cd app && cargo tauri ios dev "$(IOS_SIM)"

ios-build:       ## Build the iOS app for the simulator (unsigned debug; signing is configured in Xcode)
	cd app && cargo tauri ios build --debug --target aarch64-sim --ci

ios-run: ios-build ## Build, install and launch the iOS app in the simulator ($(IOS_SIM))
	xcrun simctl boot "$(IOS_SIM)" 2>/dev/null || true
	open -a Simulator
	xcrun simctl install "$(IOS_SIM)" $(IOS_APP)
	xcrun simctl launch "$(IOS_SIM)" sh.atelier.app

IOS_DEVICE ?=
IOS_IPA := app/gen/apple/build/arm64/Atelier.ipa

# `cargo tauri ios run` can't export to a device (tauri-cli 2.12 loses its temporary exportOptions plist), so build
# the IPA with `ios build` and install it with devicectl.
ios-device:      ## Build (release), install and launch on a connected iPhone; needs APPLE_DEVELOPMENT_TEAM, IOS_DEVICE (README)
	@test -n "$$APPLE_DEVELOPMENT_TEAM" || { echo "set APPLE_DEVELOPMENT_TEAM (README: Run on your iPhone)"; exit 1; }
	@test -n "$(IOS_DEVICE)" || { echo "set IOS_DEVICE to a name from: xcrun devicectl list devices"; exit 1; }
	cd app && cargo tauri ios build --target aarch64 --ci
	xcrun devicectl device install app --device "$(IOS_DEVICE)" $(IOS_IPA)
	xcrun devicectl device process launch --device "$(IOS_DEVICE)" sh.atelier.app

gen:             ## Regenerate TypeScript bindings from the Rust protocol types
	rm -rf web/src/gen && cargo test -q -p atelier-core -p atelier-proto export_bindings >/dev/null

# Cargo packages `test` and `check` cover. CI on Linux narrows this to the server crates
# (`make check CARGO_SCOPE="--workspace --exclude atelier-app"`): the app needs webkit2gtk, which only macOS has.
CARGO_SCOPE ?= --workspace

test:            ## All tests
	cargo test $(CARGO_SCOPE)
	pnpm -C web test

check:           ## Lint + typecheck everything (run before handing work back)
	cargo fmt --all --check
	cargo clippy $(CARGO_SCOPE) --all-targets -- -D warnings
	$(MAKE) gen && git diff --exit-code -- web/src/gen || (echo "web/src/gen is stale: commit the regenerated bindings"; exit 1)
	pnpm -C web check

fmt:             ## Format Rust
	cargo fmt --all
