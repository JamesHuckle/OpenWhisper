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
| `.cursor/codex-a2a-user-rule.txt` | User Rule text to paste into Cursor by hand |

## Setup (only if you want Codex available)

Run the `codex` commands below from your own terminal, not from Cursor Agent:
the project hook blocks every bare `codex` command in the Agent shell, including
`--version` and `login`. Set up each environment (native Windows, WSL2,
Linux/macOS) on its own.

1. Check that the six integration files in the table above are present.
2. In WSL2, use a Node installed inside WSL: `command -v node` must not be
   under `/mnt/<drive>/`. Nothing refuses a Windows Node for you, and one
   would install and run the Windows Codex against this Linux checkout.
3. Install Codex and check it runs:

   ```bash
   npm install -g @openai/codex
   codex --version
   ```

4. Sign in and check the sign-in:

   ```bash
   codex login
   codex login status
   ```

5. For a rule that is reliably visible in Cursor 3, open
   `.cursor/codex-a2a-user-rule.txt`, copy all of it, paste it into
   Settings → Rules → User Rules and save.
6. Smoke test (allowed from the Agent shell too). It must exit with status 0
   and reply `CODEX_A2A_OK`:

   ```bash
   node scripts/invoke-codex-agent.mjs --read-only --task "Do not edit files or run commands. Reply exactly: CODEX_A2A_OK"
   ```

7. Reload the Cursor window after trusting this project's hooks. Press
   Ctrl+Shift+J and check Rules, or the chat's Active Rules indicator, for
   `codex-orchestrator`; Cursor's separate Agents Window may not show
   file-backed rules even when they are active.
