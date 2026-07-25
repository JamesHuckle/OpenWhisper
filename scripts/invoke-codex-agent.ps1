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

if ($env:CURSOR_CODEX_A2A_DEPTH) {
    throw "Nested agent delegation is forbidden (CURSOR_CODEX_A2A_DEPTH is already set)."
}

$gitDir = (& git -C $repoRoot rev-parse --absolute-git-dir 2>$null)
if ($LASTEXITCODE -ne 0 -or -not $gitDir) {
    throw "The A2A wrapper must run inside a Git worktree."
}
$gitDir = $gitDir.Trim()
$lockPath = Join-Path $gitDir "cursor-codex-a2a.lock"
$stateDir = Join-Path $gitDir "cursor-codex-a2a"
$lockStream = $null
$ownsLock = $false

function Resolve-CodexInvocation {
    # On Windows, `Get-Command codex` often prefers codex.ps1 (ExternalScript).
    # Prefer .cmd → node entry so we invoke the real CLI binary.
    $candidates = @(
        (Get-Command "codex.cmd" -ErrorAction SilentlyContinue),
        (Get-Command "codex.exe" -ErrorAction SilentlyContinue),
        (Get-Command "codex" -ErrorAction SilentlyContinue)
    ) | Where-Object { $_ }

    foreach ($cmd in $candidates) {
        $source = [string]$cmd.Source
        if ($source -match '\.ps1$') {
            $siblingCmd = [System.IO.Path]::ChangeExtension($source, ".cmd")
            if (Test-Path -LiteralPath $siblingCmd) { $source = $siblingCmd }
        }
        if ($source -match '\.cmd$') {
            $js = Join-Path (Split-Path -Parent $source) "node_modules\@openai\codex\bin\codex.js"
            if (Test-Path -LiteralPath $js) {
                $node = (Get-Command node -ErrorAction Stop).Source
                return @{ Command = $node; PrefixArgs = @($js) }
            }
            return @{ Command = $source; PrefixArgs = @() }
        }
        if ($source -match '\.exe$') {
            return @{ Command = $source; PrefixArgs = @() }
        }
    }

    throw "codex is not installed (expected codex.cmd / codex.exe on PATH)."
}

function Write-StaticModelCatalog {
    param([string]$Path, [string]$Slug)
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Path) | Out-Null
    $prep = Join-Path $PSScriptRoot "prepare-codex-static-catalog.mjs"
    $node = (Get-Command node -ErrorAction Stop).Source
    & $node $prep $Path $Slug | Out-Null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $Path)) {
        throw "Failed to prepare static Codex model catalog at $Path."
    }
}

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

    if (-not $Model) {
        $Model = if ($env:CURSOR_CODEX_MODEL) { $env:CURSOR_CODEX_MODEL } else { "gpt-5.6-sol" }
    }

    $catalogPath = Join-Path $stateDir "model-catalog.json"
    Write-StaticModelCatalog -Path $catalogPath -Slug $Model
    $catalogToml = ($catalogPath -replace '\\', '/')

    $status = (& git -C $repoRoot status --short | Out-String).TrimEnd()
    if (-not $status) { $status = "(clean)" }
    $mode = if ($ReadOnly) { "read-only" } else { "workspace-write" }

    $prompt = @"
You are the sole implementation agent for this task.

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

    $invocation = Resolve-CodexInvocation
    $argList = @()
    $argList += $invocation.PrefixArgs
    $argList += @(
        "exec",
        "--cd", $repoRoot,
        "--sandbox", $mode,
        "--color", "never",
        "--model", $Model,
        # Static catalog => StaticModelsManager; skip refresh-child FD leaks
        # that kill long-running exec (models_cache TTL / schema skew).
        "--disable", "remote_models",
        "-c", "model_catalog_json=`"$catalogToml`"",
        "-"
    )

    $previousDepth = $env:CURSOR_CODEX_A2A_DEPTH
    $env:CURSOR_CODEX_A2A_DEPTH = "1"
    # Codex may still log non-fatal diagnostics to stderr. With
    # $ErrorActionPreference=Stop, redirected NativeCommandError aborts before
    # the CLI can finish — so Continue for the invoke and trust $LASTEXITCODE.
    $previousEap = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $prompt | & $invocation.Command @argList 2>&1 | ForEach-Object {
            if ($_ -is [System.Management.Automation.ErrorRecord]) {
                [Console]::Error.WriteLine($_.ToString())
            } else {
                $_
            }
        }
        $code = $LASTEXITCODE
        if ($code -ne 0) {
            $signed = if ($code -gt 2147483647) { $code - 4294967296 } else { $code }
            throw "Codex exited with code $code (signed $signed)."
        }
    } finally {
        $ErrorActionPreference = $previousEap
        $env:CURSOR_CODEX_A2A_DEPTH = $previousDepth
    }
} finally {
    if ($lockStream) { $lockStream.Dispose() }
    if ($ownsLock -and (Test-Path -LiteralPath $lockPath)) {
        Remove-Item -LiteralPath $lockPath -Force
    }
}
