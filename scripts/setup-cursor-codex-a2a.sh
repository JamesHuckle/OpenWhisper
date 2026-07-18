#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)

if ! command -v node >/dev/null 2>&1; then
  cat >&2 <<'EOF'
Node.js is required inside this WSL/Linux environment.

Install a current Node.js release using your preferred WSL package/version
manager, then install Codex natively in WSL. Do not rely on a /mnt/c Windows npm
or codex shim. After reopening the WSL terminal, rerun:

  bash ./scripts/setup-cursor-codex-a2a.sh --install --login
EOF
  exit 10
fi

node_path=$(command -v node)
case "$node_path" in
  /mnt/[a-zA-Z]/*)
    echo "Refusing Windows Node shim $node_path; install Node natively inside WSL." >&2
    exit 11
    ;;
esac

exec node "$script_dir/setup-cursor-codex-a2a.mjs" "$@"

