#!/usr/bin/env bash
set -euo pipefail

task=""
read_only=0
model=""

while (($#)); do
  case "$1" in
    --task|-Task)
      [[ $# -ge 2 ]] || { echo "--task requires a value" >&2; exit 2; }
      task=$2
      shift 2
      ;;
    --read-only|-ReadOnly)
      read_only=1
      shift
      ;;
    --model|-Model)
      [[ $# -ge 2 ]] || { echo "--model requires a value" >&2; exit 2; }
      model=$2
      shift 2
      ;;
    --)
      shift
      break
      ;;
    *)
      echo "Unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

[[ -n "$task" ]] || { echo "--task is required" >&2; exit 2; }
[[ -z "${OPENWHISPER_CODEX_A2A_DEPTH:-}" ]] || {
  echo "Nested agent delegation is forbidden (OPENWHISPER_CODEX_A2A_DEPTH is already set)." >&2
  exit 3
}

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
repo_root=$(cd -- "$script_dir/.." && pwd -P)
git_dir=$(git -C "$repo_root" rev-parse --absolute-git-dir 2>/dev/null) || {
  echo "The A2A wrapper must run inside the OpenWhisper Git worktree." >&2
  exit 4
}

# mkdir is atomic on both Linux filesystems and /mnt/c, and conflicts with the
# same path used by the Windows FileMode.CreateNew lock.
lock_path="$git_dir/openwhisper-codex-a2a.lock"
if ! mkdir -- "$lock_path" 2>/dev/null; then
  echo "Another Codex delegation is active, or a stale lock exists at $lock_path." >&2
  echo "Verify no Codex task is running before removing the stale lock." >&2
  exit 5
fi
cleanup() { rmdir -- "$lock_path" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
printf 'pid=%s\nstarted=%s\nenvironment=wsl-linux\n' "$$" "$(date --iso-8601=seconds)" > "$lock_path/owner"

command -v codex >/dev/null 2>&1 || {
  echo "codex is not installed in this WSL distribution or is not on PATH." >&2
  exit 6
}

status=$(git -C "$repo_root" status --short)
[[ -n "$status" ]] || status="(clean)"
sandbox="workspace-write"
((read_only)) && sandbox="read-only"

prompt=$(cat <<EOF
You are the sole implementation agent for this OpenWhisper task.

TASK
$task

CONTRACT
- Work only in: $repo_root
- Preserve unrelated user changes; the starting git status is included below.
- Do not invoke Cursor, Codex, invoke-codex-agent.ps1, invoke-codex-agent.sh, or another agent.
- Do not commit, push, open pull requests, or modify Git configuration.
- Inspect before editing, implement the task completely, and run proportionate verification.
- End with a concise list of changed files, verification performed, and remaining risks.

STARTING GIT STATUS
$status
EOF
)

args=(exec --cd "$repo_root" --sandbox "$sandbox" --color never)
[[ -z "$model" ]] || args+=(--model "$model")
args+=(-)

export OPENWHISPER_CODEX_A2A_DEPTH=1
printf '%s\n' "$prompt" | codex "${args[@]}"

