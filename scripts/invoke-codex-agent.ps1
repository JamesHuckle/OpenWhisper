[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$Task,

    [switch]$ReadOnly,

    [string]$Model
)

$ErrorActionPreference = "Stop"
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path

if ($env:OPENWHISPER_CODEX_A2A_DEPTH) {
    throw "Nested agent delegation is forbidden (OPENWHISPER_CODEX_A2A_DEPTH is already set)."
}

$gitDir = (& git -C $repoRoot rev-parse --absolute-git-dir 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $gitDir) {
    throw "The A2A wrapper must run inside the OpenWhisper Git worktree."
}
$gitDir = $gitDir.Trim()
$lockPath = Join-Path $gitDir "openwhisper-codex-a2a.lock"
$lockStream = $null
$ownsLock = $false

try {
    try {
        $lockStream = [System.IO.File]::Open(
            $lockPath,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None
        )
        $ownsLock = $true
    } catch [System.IO.IOException] {
        throw "Another Codex delegation is active, or a stale lock exists at $lockPath. Verify no Codex task is running before removing it."
    }

    $lockText = [Text.Encoding]::UTF8.GetBytes("pid=$PID`nstarted=$([DateTimeOffset]::Now.ToString('o'))`n")
    $lockStream.Write($lockText, 0, $lockText.Length)
    $lockStream.Flush()

    $codex = Get-Command codex -ErrorAction Stop
    $status = (& git -C $repoRoot status --short | Out-String).TrimEnd()
    if (-not $status) { $status = "(clean)" }
    $mode = if ($ReadOnly) { "read-only" } else { "workspace-write" }

    $prompt = @"
You are the sole implementation agent for this OpenWhisper task.

TASK
$Task

CONTRACT
- Work only in: $repoRoot
- Preserve unrelated user changes; the starting git status is included below.
- Do not invoke Cursor, Codex, invoke-codex-agent.ps1, or another agent.
- Do not commit, push, open pull requests, or modify Git configuration.
- Inspect before editing, implement the task completely, and run proportionate verification.
- End with a concise list of changed files, verification performed, and remaining risks.

STARTING GIT STATUS
$status
"@

    $args = @("exec", "--cd", $repoRoot, "--sandbox", $mode, "--color", "never")
    if ($Model) { $args += @("--model", $Model) }
    $args += "-"

    $previousDepth = $env:OPENWHISPER_CODEX_A2A_DEPTH
    $env:OPENWHISPER_CODEX_A2A_DEPTH = "1"
    try {
        $prompt | & $codex.Source @args
        if ($LASTEXITCODE -ne 0) {
            throw "Codex exited with code $LASTEXITCODE."
        }
    } finally {
        $env:OPENWHISPER_CODEX_A2A_DEPTH = $previousDepth
    }
} finally {
    if ($lockStream) { $lockStream.Dispose() }
    if ($ownsLock -and (Test-Path -LiteralPath $lockPath)) {
        Remove-Item -LiteralPath $lockPath -Force
    }
}
