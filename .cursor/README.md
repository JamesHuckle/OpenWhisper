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
child timeouts / FD leaks). On Windows the wrapper tolerates Codex stderr noise
so PowerShell cannot abort the run early.

Default model: `gpt-5.6-sol` (override with `-Model` / `CURSOR_CODEX_MODEL`).

## Optional Codex invocation

```powershell
& .\scripts\invoke-codex-agent.ps1 -Task "Add the requested feature and run its tests"
```

From a Cursor window connected to WSL2:

```bash
bash ./scripts/invoke-codex-agent.sh --task "Add the requested feature and run its tests"
```

Use `-ReadOnly` / `--read-only` for analysis that must not edit. Install and
authenticate Codex separately in each environment (`codex login`).

## Layout

| Path | Role |
|------|------|
| `.cursor/rules/codex-orchestrator.mdc` | Always-on: Cursor-first policy |
| `.cursor/commands/delegate-to-codex.md` | Slash command for opt-in handoff |
| `.cursor/hooks.json` + `hooks/enforce-codex-wrapper.js` | Block bare `codex` in Agent shell |
| `scripts/invoke-codex-agent.ps1` / `.sh` | Checked launcher + lock |
| `scripts/prepare-codex-static-catalog.mjs` | Static model catalog for long exec |
| `scripts/setup-cursor-codex-a2a.*` | Install / login helpers |

## Setup (only if you want Codex available)

```powershell
& .\scripts\setup-cursor-codex-a2a.ps1 -Install -Login
```

```bash
bash ./scripts/setup-cursor-codex-a2a.sh --install --login
```

Optional User Rule copy for Cursor 3 UI visibility:

```powershell
& .\scripts\setup-cursor-codex-a2a.ps1 -CopyUserRule
```
