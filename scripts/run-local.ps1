$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot\..

# Ensure Cargo is in PATH (Git Bash / some terminals don't pick it up)
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}

# Ensure .env exists
if (-not (Test-Path .env)) {
    Copy-Item .env.example .env
    Write-Host "Created .env from .env.example"
}

# Sync Python worker
Push-Location apps\worker
uv sync
Pop-Location

# Run desktop app
Push-Location apps\desktop
npm install
npm run tauri:dev
Pop-Location
