# One-time setup for Tauri updater signing keys.
# Run from repo root. Generates keys and injects the public key into tauri.conf.json.
#
# Usage: .\scripts\setup-updater-keys.ps1

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Split-Path -Parent $PSScriptRoot)).Path `
  -replace '^Microsoft\.PowerShell\.Core\\FileSystem::', ''

$KeyDir = Join-Path $env:USERPROFILE ".tauri"
$KeyPath = Join-Path $KeyDir "openwhisper.key"
$PubPath = "$KeyPath.pub"
$TauriConfPath = Join-Path $RepoRoot "apps/desktop/src-tauri/tauri.conf.json"

if (-not (Test-Path $KeyPath)) {
  Write-Host "==> Generating updater signing keys at $KeyDir"
  Push-Location (Join-Path $RepoRoot "apps/desktop")
  npx tauri signer generate -w $KeyPath
  Pop-Location
  Write-Host ""
  Write-Host "Keys created. Store the PRIVATE key safely - you need it for every release."
  Write-Host "  Private: $KeyPath"
  Write-Host "  Public:  $PubPath"
  Write-Host ""
} else {
  Write-Host "==> Keys already exist at $KeyPath"
}

if (-not (Test-Path $PubPath)) {
  throw "Public key not found: $PubPath"
}

$pubContent = (Get-Content $PubPath -Raw).Trim()
$escaped = $pubContent -replace '\\', '\\\\' -replace '"', '\"' -replace "`r`n", '\n' -replace "`n", '\n' -replace "`r", ''
$confRaw = Get-Content $TauriConfPath -Raw
$confRaw = $confRaw -replace 'REPLACE_WITH_PUBLIC_KEY', $escaped
Set-Content -Path $TauriConfPath -Value $confRaw -NoNewline

Write-Host "==> Injected public key into tauri.conf.json"

# Add private key to .env (all secrets in one place)
$EnvPath = Join-Path $RepoRoot ".env"
$privContent = (Get-Content $KeyPath -Raw).Trim()
$privEscaped = $privContent -replace '\\', '\\\\' -replace '"', '\"' -replace "`r`n", '\n' -replace "`n", '\n' -replace "`r", ''
$envLine = "TAURI_SIGNING_PRIVATE_KEY=`"$privEscaped`""

if (Test-Path $EnvPath) {
  $existing = Get-Content $EnvPath -Raw
  if ($existing -match 'TAURI_SIGNING_PRIVATE_KEY=') {
    $existing = $existing -replace 'TAURI_SIGNING_PRIVATE_KEY="[^"]*"', $envLine
  } else {
    $existing = $existing.TrimEnd() + "`n`n$envLine`n"
  }
  Set-Content -Path $EnvPath -Value $existing -NoNewline
} else {
  Set-Content -Path $EnvPath -Value $envLine -NoNewline
}
Write-Host "==> Added TAURI_SIGNING_PRIVATE_KEY to .env"
Write-Host ""
Write-Host "Setup complete. All secrets (including signing key) are in .env"
Write-Host ""
