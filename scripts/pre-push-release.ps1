$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Split-Path -Parent $PSScriptRoot)).Path `
  -replace '^Microsoft\.PowerShell\.Core\\FileSystem::', ''

# ── Preflight ──────────────────────────────────────────────────────────────────

foreach ($cmd in @("gh", "cargo")) {
  if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
    Write-Host "==> Skipping release: '$cmd' not found in PATH"
    exit 0
  }
}

$authStatus = gh auth status 2>&1
if ($LASTEXITCODE -ne 0) {
  Write-Host "==> Skipping release: gh is not authenticated (run 'gh auth login')"
  exit 0
}

# ── Read version ───────────────────────────────────────────────────────────────

$tauriConf = Get-Content (Join-Path $RepoRoot "apps/desktop/src-tauri/tauri.conf.json") -Raw | ConvertFrom-Json
$version = $tauriConf.version
$productName = $tauriConf.productName
$tag = "v$version"

Write-Host ""
Write-Host "╔══════════════════════════════════════════╗"
Write-Host "║  Building release: $productName $tag"
Write-Host "╚══════════════════════════════════════════╝"
Write-Host ""

# ── Build the Windows installer ────────────────────────────────────────────────

& "$PSScriptRoot/build-windows-installer.ps1"

# ── Locate installer ──────────────────────────────────────────────────────────

$CargoTargetDir = Join-Path $env:USERPROFILE "openwhisper-cargo-target"
$InstallerDir = Join-Path $CargoTargetDir "release/bundle/nsis"
$VersionedInstaller = Join-Path $InstallerDir "${productName}_${version}_x64-setup.exe"

if (-not (Test-Path $VersionedInstaller)) {
  throw "Installer not found: $VersionedInstaller"
}

$StableInstaller = Join-Path $InstallerDir "${productName}_x64-setup.exe"
Copy-Item $VersionedInstaller $StableInstaller -Force

# ── Tag and push (with recursion guard) ────────────────────────────────────────

Write-Host "==> Tagging $tag"
git tag -f $tag

$env:OPENWHISPER_RELEASING = "1"
git push origin $tag --force
$env:OPENWHISPER_RELEASING = $null

if ($LASTEXITCODE -ne 0) { throw "Failed to push tag" }

# ── Create / update GitHub Release ─────────────────────────────────────────────

Write-Host "==> Publishing GitHub Release $tag"

gh release view $tag 2>$null | Out-Null
if ($LASTEXITCODE -ne 0) {
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
