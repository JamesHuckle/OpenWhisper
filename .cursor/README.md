# Cursor → Codex orchestration

This project lets Cursor work normally while strongly preferring the locally
authenticated Codex CLI for substantial planning, implementation, debugging,
refactoring, and review.

- The always-on advisory rule recommends the native Windows
  `scripts/invoke-codex-agent.ps1` or WSL/Linux `scripts/invoke-codex-agent.sh`
  for substantial work while allowing Cursor to handle routine tasks directly.
- The shell hook blocks direct `codex` launches from Cursor Agent.
- The wrapper fixes the repository root and sandbox, prevents nested or
  concurrent write agents, supplies dirty-worktree context, and propagates the
  Codex process exit code.

Example manual invocation:

```powershell
& .\scripts\invoke-codex-agent.ps1 -Task "Add the requested feature and run its tests"
```

From a Cursor window connected to WSL2:

```bash
bash ./scripts/invoke-codex-agent.sh --task "Add the requested feature and run its tests"
```

Use `-ReadOnly` on Windows or `--read-only` on WSL for analysis that must not
edit the workspace. Install and authenticate Codex separately inside the WSL
distribution (`codex login`); Windows and WSL do not necessarily share CLI
credentials or PATH configuration.

Cursor 2.1 removed Custom Modes. The supported setup therefore uses the
always-on advisory project rule with every normal Agent model; Cursor retains
all of its tools and decides whether a task is routine or substantial. Use
`/delegate-to-codex` when you want to force a complete handoff for one task.

To inspect the active rule, use the editor Settings (`Ctrl+Shift+J`) → Rules,
the chat's Active Rules indicator, or reference `@codex-orchestrator` in chat.
Cursor 3's separate Agents Window has a known display bug where file-backed
rules may be active without appearing in its Rules screen. To temporarily
disable the preference, change `alwaysApply` to `false`; the explicit slash
command remains available.

For a rule that is reliably visible in the Cursor 3 UI, copy the committed,
repository-aware rule and paste it into Settings → Rules → User Rules:

```powershell
& .\scripts\setup-cursor-codex-a2a.ps1 -CopyUserRule
```

```bash
bash ./scripts/setup-cursor-codex-a2a.sh --copy-user-rule
```

The User Rule is global because Cursor stores it in its synced settings, but its
instructions activate only when the current workspace contains one of this
repository's checked wrapper scripts. The committed `.mdc` remains the complete
project-scoped source of truth.

Cursor rules are behavioral controls, not a security boundary. The hook is the
deterministic control for direct CLI launches, but a user can still disable
project hooks or execute commands outside Cursor. Keep Cursor current and leave
`failClosed` enabled.

## Cross-platform setup

Run the committed setup assistant from native Windows:

```powershell
& .\scripts\setup-cursor-codex-a2a.ps1 -Install -Login
```

Or from a Cursor window connected to WSL2:

```bash
bash ./scripts/setup-cursor-codex-a2a.sh --install --login
```

Node and Codex must be installed natively in each environment. In particular,
WSL must not resolve them through `/mnt/c` Windows shims. The WSL bootstrap
detects that condition and prints remediation. Omit the install/login flags when
already configured. Both launchers call the same committed cross-platform setup
assistant and validate the repository integration and authenticated Codex CLI.
