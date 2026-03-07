param(
  [string]$LocalBuildRoot
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Split-Path -Parent $PSScriptRoot)).Path `
  -replace '^Microsoft\.PowerShell\.Core\\FileSystem::', ''

# ── Preflight ──────────────────────────────────────────────────────────────────

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
  throw @"
GitHub CLI (gh) is required for releasing.
Install it:  winget install --id GitHub.cli
Then auth:   gh auth login
"@
}

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
  throw "git not found in PATH."
}

# ── Read version from Tauri config ─────────────────────────────────────────────

$tauriConf = Get-Content (Join-Path $RepoRoot "apps/desktop/src-tauri/tauri.conf.json") -Raw | ConvertFrom-Json
$version = $tauriConf.version
$productName = $tauriConf.productName
$tag = "v$version"

Write-Host ""
Write-Host "╔══════════════════════════════════════════╗"
Write-Host "║  Releasing $productName $tag"
Write-Host "╚══════════════════════════════════════════╝"
Write-Host ""

# ── Push code to origin ───────────────────────────────────────────────────────

Write-Host "==> Pushing code to origin"
git push origin HEAD
if ($LASTEXITCODE -ne 0) { throw "git push failed" }

# ── Build the Windows installer ────────────────────────────────────────────────

Write-Host "==> Building Windows installer"
$buildArgs = @()
if ($LocalBuildRoot) {
  $buildArgs += "-LocalBuildRoot", $LocalBuildRoot
}
& "$PSScriptRoot/build-windows-installer.ps1" @buildArgs

# ── Locate installer artifacts ─────────────────────────────────────────────────

$CargoTargetDir = Join-Path $env:USERPROFILE "openwhisper-cargo-target"
$InstallerDir = Join-Path $CargoTargetDir "release/bundle/nsis"
$VersionedInstaller = Join-Path $InstallerDir "${productName}_${version}_x64-setup.exe"

if (-not (Test-Path $VersionedInstaller)) {
  throw "Installer not found: $VersionedInstaller"
}

$StableInstaller = Join-Path $InstallerDir "${productName}_x64-setup.exe"
Copy-Item $VersionedInstaller $StableInstaller -Force

Write-Host "==> Installer ready:"
Write-Host "    Versioned: $VersionedInstaller"
Write-Host "    Stable:    $StableInstaller"

# ── Tag and push ───────────────────────────────────────────────────────────────

Write-Host "==> Tagging $tag"
git tag -f $tag
git push origin $tag --force
if ($LASTEXITCODE -ne 0) { throw "Failed to push tag" }

# ── Create / update GitHub Release ─────────────────────────────────────────────

Write-Host "==> Publishing GitHub Release $tag"

$releaseExists = $false
gh release view $tag 2>$null | Out-Null
if ($LASTEXITCODE -eq 0) { $releaseExists = $true }

if (-not $releaseExists) {
  gh release create $tag `
    --title "$productName $tag" `
    --generate-notes `
    --latest
  if ($LASTEXITCODE -ne 0) { throw "Failed to create release" }
}

gh release upload $tag $VersionedInstaller $StableInstaller --clobber
if ($LASTEXITCODE -ne 0) { throw "Failed to upload assets" }

gh release edit $tag --latest
if ($LASTEXITCODE -ne 0) { Write-Host "    Warning: could not mark release as latest" }

# ── Done ───────────────────────────────────────────────────────────────────────

Write-Host ""
Write-Host "╔══════════════════════════════════════════╗"
Write-Host "║  Released $productName $tag"
Write-Host "╚══════════════════════════════════════════╝"
Write-Host ""
Write-Host "  GitHub:   https://github.com/JamesHuckle/OpenWhisper/releases/tag/$tag"
Write-Host "  Download: https://github.com/JamesHuckle/OpenWhisper/releases/latest/download/${productName}_x64-setup.exe"
Write-Host ""
