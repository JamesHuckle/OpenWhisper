$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot\..

# Keep custom Cargo output outside src-tauri. Tauri watches that source tree,
# so a target directory placed there creates an endless rebuild/relaunch loop.
$env:CARGO_TARGET_DIR = [IO.Path]::GetFullPath((Join-Path (Get-Location) "target\desktop-dev"))

# Ensure Cargo is in PATH (Git Bash / some terminals don't pick it up)
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}

# Run desktop app
Push-Location apps\desktop
npm install
npm run tauri:dev
Pop-Location
