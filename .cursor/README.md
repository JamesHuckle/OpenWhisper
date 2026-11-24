# Cursor ↔ Codex (optional)

Shareable source for other projects:
[JamesHuckle/cursor-codex-a2a](https://github.com/JamesHuckle/cursor-codex-a2a)
(`node install.mjs --target <project>`).

**Default:** Cursor edits this repo live with its normal tools.

**Opt-in:** `/delegate-to-codex` or an explicit “use Codex” request runs the
checked wrapper. Bare `codex` launches from the Agent shell are still blocked by
the project hook so accidental bypasses do not happen.

## Why Cursor-first

Codex `exec` is a separate process: no live IDE streaming diffs (files still land
on disk incrementally). Prefer watching Cursor patch files in place for most
work.

The checked wrapper hardens long runs: pinned `--model`, `--disable
remote_models`, and a static `model_catalog_json` (avoids models-cache refresh
child timeouts / FD leaks).

Default model: `gpt-5.6-sol` (override with `--model` / `CURSOR_CODEX_MODEL`).

## Optional Codex invocation

The same command works in native Windows, WSL2 and Linux/macOS windows:

```bash
node scripts/invoke-codex-agent.mjs --task "Add the requested feature and run its tests"
```

Use `--read-only` for analysis that must not edit (`--help` lists every
option). Install and authenticate Codex separately in each environment
(`codex login`).

## Layout

| Path | Role |
|------|------|
| `.cursor/rules/codex-orchestrator.mdc` | Always-on: Cursor-first policy |
| `.cursor/commands/delegate-to-codex.md` | Slash command for opt-in handoff |
| `.cursor/hooks.json` + `hooks/enforce-codex-wrapper.js` | Block bare `codex` in Agent shell |
| `scripts/invoke-codex-agent.mjs` | Checked launcher + lock + static model catalog for long exec |
| `scripts/setup-cursor-codex-a2a.mjs` | Install / login helper |

## Setup (only if you want Codex available)

```bash
node scripts/setup-cursor-codex-a2a.mjs --install --login
```

In WSL2, use a Node installed inside WSL; the setup refuses a Windows Node
found under `/mnt/<drive>/`.

Optional User Rule copy for Cursor 3 UI visibility:

```bash
node scripts/setup-cursor-codex-a2a.mjs --copy-user-rule
```
