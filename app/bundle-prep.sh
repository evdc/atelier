#!/bin/sh
# beforeBuildCommand for release bundles (`make bundle`): build the web UI and the server, and put the server where
# Tauri expects a sidecar: binaries/atelierd-<target triple>. Kept out of tauri.conf.json because tauri-build
# insists the sidecar exists at every build, dev and iOS included.
set -e
cd "$(dirname "$0")"
triple="${TAURI_ENV_TARGET_TRIPLE:-$(rustc -vV | sed -n 's/^host: //p')}"
pnpm -C ../web build
cargo build --release -p atelier-server --target "$triple"
mkdir -p binaries
cp "../target/$triple/release/atelierd" "binaries/atelierd-$triple"
