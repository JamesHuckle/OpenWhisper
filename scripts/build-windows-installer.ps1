param(
  [switch]$SkipNpmInstall,
  [string]$LocalBuildRoot
)

$ErrorActionPreference = "Stop"

$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
}

function Import-VsBuildToolsEnvironment {
  if (Get-Command link.exe -ErrorAction SilentlyContinue) {
    return
  }

  $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
  $installPath = $null
  if (Test-Path $vswhere) {
    $installPath = (& $vswhere `
      -latest `
      -products * `
      -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
      -property installationPath | Select-Object -First 1).Trim()
  }

  $vsDevCmdCandidates = @()
  if ($installPath) {
    $vsDevCmdCandidates += (Join-Path $installPath "Common7\Tools\VsDevCmd.bat")
  }
  $vsDevCmdCandidates += @(
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat",
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat",
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\Professional\Common7\Tools\VsDevCmd.bat",
    "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\Enterprise\Common7\Tools\VsDevCmd.bat"
  )

  $vsDevCmd = $vsDevCmdCandidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
  if ($vsDevCmd) {
    Write-Host "==> Loading Visual Studio C++ build environment"
    $envDump = & cmd.exe /d /s /c "`"$vsDevCmd`" -arch=x64 -host_arch=x64 >nul && set"
    foreach ($line in $envDump) {
      if ($line -match "^(.*?)=(.*)$") {
        Set-Item -Path "env:$($matches[1])" -Value $matches[2]
      }
    }
  }

  if (-not (Get-Command link.exe -ErrorAction SilentlyContinue) -and $installPath) {
    $linkCandidate = Get-ChildItem -Path (Join-Path $installPath "VC\Tools\MSVC") -Filter link.exe -Recurse -ErrorAction SilentlyContinue |
      Where-Object { $_.FullName -match "Hostx64\\x64\\link\.exe$" } |
      Select-Object -First 1
    if ($linkCandidate) {
      $linkDir = Split-Path -Parent $linkCandidate.FullName
      $env:Path = "$linkDir;$env:Path"
    }
  }
}

$OriginalRepoRoot = (Resolve-Path (Split-Path -Parent $PSScriptRoot)).Path `
  -replace '^Microsoft\.PowerShell\.Core\\FileSystem::', ''
$RepoRoot = $OriginalRepoRoot
$IsUncRepo = $RepoRoot.StartsWith("\\") -or $RepoRoot.ToLowerInvariant().Contains("\\wsl$\")
$BuildRoot = $RepoRoot

if ($IsUncRepo) {
  if ([string]::IsNullOrWhiteSpace($LocalBuildRoot)) {
    $BuildRoot = Join-Path $env:USERPROFILE "openwhisper-win-build"
  } else {
    $BuildRoot = $LocalBuildRoot
  }
  Write-Host "==> UNC/WSL path detected. Mirroring repo to local Windows path:"
  Write-Host "    $BuildRoot"

  if (-not (Test-Path $BuildRoot)) {
    New-Item -ItemType Directory -Path $BuildRoot | Out-Null
  }

  # Build from local NTFS path to avoid UNC/cmd issues and Linux node_modules artifacts.
  # /MIR with /XD preserves excluded dirs (target, node_modules) across runs for
  # incremental Rust builds and to avoid Application Control blocking re-compiled binaries.
  robocopy $RepoRoot $BuildRoot /MIR /R:1 /W:1 /NFL /NDL /NJH /NJS /NP `
    /XD ".git" "node_modules" ".venv" "target" "dist" "build" ".mypy_cache" ".pytest_cache" | Out-Null

  if ($LASTEXITCODE -gt 7) {
    throw "robocopy failed with exit code $LASTEXITCODE"
  }
}

if ($BuildRoot.ToLowerInvariant().Contains("\appdata\local\temp\")) {
  Write-Host "==> Warning: build root is under TEMP and may be blocked by Application Control policies."
  Write-Host "    Consider -LocalBuildRoot C:\Users\$env:USERNAME\openwhisper-win-build"
}

$WorkerDir = Join-Path $BuildRoot "apps/worker"
$DesktopDir = Join-Path $BuildRoot "apps/desktop"
$BinariesDir = Join-Path $DesktopDir "src-tauri/binaries"

# Keep the Rust target dir outside the mirrored build root so it persists across runs.
# This avoids Application Control policies blocking freshly-compiled build-script binaries
# every time the build root is re-mirrored.
$CargoTargetDir = Join-Path $env:USERPROFILE "openwhisper-cargo-target"
if (-not (Test-Path $CargoTargetDir)) {
  New-Item -ItemType Directory -Path $CargoTargetDir | Out-Null
}
$env:CARGO_TARGET_DIR = $CargoTargetDir
Write-Host "==> Rust target dir: $CargoTargetDir"

Write-Host "==> Preflight checks"
$cargoCmd = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargoCmd) {
  throw @"
Rust/Cargo was not found in PATH.
Install Rust for Windows, then reopen PowerShell and rerun:
  winget install -e --id Rustlang.Rustup
or:
  https://rustup.rs/
"@
}

Write-Host "    cargo: $($cargoCmd.Source)"
Import-VsBuildToolsEnvironment
$linkCmd = Get-Command link.exe -ErrorAction SilentlyContinue
if (-not $linkCmd) {
  throw @"
MSVC linker (link.exe) was not found.
Install Visual Studio Build Tools with C++ workload, then rerun:
  winget install -e --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
"@
}
Write-Host "    link.exe: $($linkCmd.Source)"

Write-Host "==> Sync worker dependencies"
Push-Location -LiteralPath $WorkerDir
if (Test-Path ".venv") {
  Write-Host "==> Removing existing worker .venv"
  try {
    Remove-Item ".venv" -Recurse -Force -ErrorAction Stop
  } catch {
    cmd /c "rmdir /s /q .venv" | Out-Null
  }
  if (Test-Path ".venv") {
    throw "Failed to remove worker .venv from build directory"
  }
}
uv sync

Write-Host "==> Build standalone worker executable"
uv run pyinstaller `
  --noconfirm `
  --onefile `
  --name openwhisper-worker `
  --paths src `
  --distpath $BinariesDir `
  src/openwhisper_worker/__main__.py
Pop-Location

Push-Location -LiteralPath $DesktopDir
if (-not $SkipNpmInstall) {
  if (Test-Path "node_modules") {
    Write-Host "==> Removing existing node_modules"
    try {
      Remove-Item "node_modules" -Recurse -Force -ErrorAction Stop
    } catch {
      cmd /c "rmdir /s /q node_modules" | Out-Null
    }
    if (Test-Path "node_modules") {
      throw "Failed to remove node_modules from build directory"
    }
  }
  Write-Host "==> Install desktop dependencies"
  if (Test-Path "package-lock.json") {
    npm ci
  } else {
    npm install
  }
}

Write-Host "==> Build Tauri Windows installer (NSIS)"
npm run tauri:build
Pop-Location

$InstallerDir = Join-Path $CargoTargetDir "release/bundle/nsis"

if ($IsUncRepo) {
  $OutDir = Join-Path $OriginalRepoRoot "artifacts/windows-installer"
  if (-not (Test-Path $InstallerDir)) {
    throw "Installer directory not found: $InstallerDir"
  }
  if (Test-Path $OutDir) {
    Remove-Item $OutDir -Recurse -Force
  }
  New-Item -ItemType Directory -Path $OutDir | Out-Null
  Copy-Item (Join-Path $InstallerDir "*") -Destination $OutDir -Recurse -Force
  Write-Host "==> Copied installer artifacts back to:"
  Write-Host "    $OutDir"
}

Write-Host ""
Write-Host "Installer artifacts:"
Write-Host $InstallerDir
