Opt-in Codex handoff. Default remains Cursor editing files in this workspace.

Only if the user wants Codex for this turn, call the checked wrapper with a
self-contained task and acceptance criteria:

```bash
node scripts/invoke-codex-agent.mjs --task '<task and acceptance criteria>'
```

The same command works on native Windows and WSL/Remote Linux. Never launch
`codex` directly. If the wrapper fails, report the error and finish the work in Cursor
unless the user forbids that.

After Codex returns, inspect the diff, verify, and summarize.
