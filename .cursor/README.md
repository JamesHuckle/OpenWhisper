# Cursor → Codex orchestration

This project configures Cursor as a thin orchestration layer over the locally
authenticated Codex CLI.

- The always-on rule requires write tasks to use
  `scripts/invoke-codex-agent.ps1`.
- The shell hook blocks direct `codex` launches from Cursor Agent.
- The wrapper fixes the repository root and sandbox, prevents nested or
  concurrent write agents, supplies dirty-worktree context, and propagates the
  Codex process exit code.

Example manual invocation:

```powershell
& .\scripts\invoke-codex-agent.ps1 -Task "Add the requested feature and run its tests"
```

Use `-ReadOnly` for analysis that must not edit the workspace.

In Cursor chat, `/delegate-to-codex` provides an explicit entry point. For the
strongest UI configuration, create a Custom Mode with only Read/Search and
Terminal enabled; disable Edit/Delete and enable this project's rules. Cursor
currently stores Custom Mode tool selection in user settings rather than this
repository, so that final toggle cannot be committed here.

Cursor rules are behavioral controls, not a security boundary. The hook is the
deterministic control for direct CLI launches, but a user can still disable
project hooks or execute commands outside Cursor. Keep Cursor current and leave
`failClosed` enabled.
