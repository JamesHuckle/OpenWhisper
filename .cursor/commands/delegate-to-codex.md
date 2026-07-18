Act as the Cursor orchestration layer defined by the always-on Codex delegation
rule. Convert the user's current request into a complete, self-contained task and
run it through:

```powershell
& .\scripts\invoke-codex-agent.ps1 -Task '<task and acceptance criteria>'
```

When the active Cursor window is connected to WSL/Remote Linux, use this instead:

```bash
bash ./scripts/invoke-codex-agent.sh --task '<task and acceptance criteria>'
```

Do not edit repository files yourself and do not invoke Codex directly. After
Codex returns, inspect the diff, run appropriate verification, and report the
result. Delegate any required correction through the wrapper as a follow-up.
