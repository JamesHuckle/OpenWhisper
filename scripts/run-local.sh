#!/usr/bin/env bash
set -e
cd "$(dirname "$0")/.."

# Keep custom Cargo output outside src-tauri, which Tauri watches for changes.
export CARGO_TARGET_DIR="$(pwd)/target/desktop-dev"

# Ensure .env exists
[ -f .env ] || { cp .env.example .env && echo "Created .env from .env.example"; }

# Sync Python worker
(cd apps/worker && uv sync)

# Run desktop app
(cd apps/desktop && npm install && npm run tauri:dev)
