#!/usr/bin/env bash
set -e
cd "$(dirname "$0")/.."

# Keep custom Cargo output outside src-tauri, which Tauri watches for changes.
export CARGO_TARGET_DIR="$(pwd)/target/desktop-dev"

# Run desktop app
(cd apps/desktop && npm install && npm run tauri:dev)
