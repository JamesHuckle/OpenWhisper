Opt-in Codex handoff. Default remains Cursor editing files in this workspace.

Only if the user wants Codex for this turn, call the platform-appropriate wrapper
with a self-contained task and acceptance criteria:

```powershell
& .\scripts\invoke-codex-agent.ps1 -Task '<task and acceptance criteria>'
```

```bash
bash ./scripts/invoke-codex-agent.sh --task '<task and acceptance criteria>'
```

Use PowerShell on native Windows; Bash on WSL/Remote Linux. Never launch `codex`
directly. If the wrapper fails, report the error and finish the work in Cursor
unless the user forbids that.

After Codex returns, inspect the diff, verify, and summarize.
