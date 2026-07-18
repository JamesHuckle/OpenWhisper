[CmdletBinding()]
param(
    [switch]$Install,
    [switch]$Login,
    [switch]$SmokeTest,
    [switch]$CopyUserRule
)

$ErrorActionPreference = "Stop"
$setupScript = Join-Path $PSScriptRoot "setup-cursor-codex-a2a.mjs"
if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
    throw "Node.js is required. Install a current Node.js release, reopen the terminal, and rerun this script."
}

$arguments = @($setupScript)
if ($Install) { $arguments += "--install" }
if ($Login) { $arguments += "--login" }
if ($SmokeTest) { $arguments += "--smoke-test" }
if ($CopyUserRule) { $arguments += "--copy-user-rule" }
& node @arguments
exit $LASTEXITCODE
