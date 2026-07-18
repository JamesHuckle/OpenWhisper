param(
  [string]$LocalBuildRoot
)

$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Split-Path -Parent $PSScriptRoot)).Path `
  -replace '^Microsoft\.PowerShell\.Core\\FileSystem::', ''

# Load .env (all secrets in one place)
. "$PSScriptRoot/load-env.ps1" -Path (Join-Path $RepoRoot ".env")

# Skip update notification if OPENWHISPER_SKIP_UPDATE_NOTIFY=1 in .env
$NotifyUpdate = $env:OPENWHISPER_SKIP_UPDATE_NOTIFY -ne "1"

# GitHub repo (override with env OPENWHISPER_GITHUB_REPO)
$GitHubRepo = if ($env:OPENWHISPER_GITHUB_REPO) { $env:OPENWHISPER_GITHUB_REPO } else { "JamesHuckle/OpenWhisper" }

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
if (-not $NotifyUpdate) { Write-Host "║  (no update notification to users)" }
Write-Host "╚══════════════════════════════════════════╝"
Write-Host ""

# ── Updater signing key ────────────────────────────────────────────────────────

$KeyPath = Join-Path $env:USERPROFILE ".tauri\openwhisper.key"
if (-not $env:TAURI_SIGNING_PRIVATE_KEY -and (Test-Path $KeyPath)) {
  $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $KeyPath -Raw
}
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
  throw @"
Updater signing key required. Run once:
  .\scripts\setup-updater-keys.ps1

Then add TAURI_SIGNING_PRIVATE_KEY to .env (or it will load from ~/.tauri/openwhisper.key)
"@
}

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
$SigPath = Join-Path $InstallerDir "${productName}_${version}_x64-setup.exe.sig"

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

$uploadAssets = @($VersionedInstaller, $StableInstaller)

if ($NotifyUpdate -and (Test-Path $SigPath)) {
  $pubDate = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
  $sigContent = (Get-Content $SigPath -Raw).Trim()
  $installerUrl = "https://github.com/$GitHubRepo/releases/download/$tag/${productName}_${version}_x64-setup.exe"
  $latestJson = @{
    version    = $version
    notes      = ""
    pub_date   = $pubDate
    platforms  = @{
      "windows-x86_64" = @{
        signature = $sigContent
        url       = $installerUrl
      }
    }
  } | ConvertTo-Json -Depth 5 -Compress
  $latestJsonPath = Join-Path $env:TEMP "latest.json"
  Set-Content -Path $latestJsonPath -Value $latestJson -NoNewline
  $uploadAssets += $latestJsonPath
  Write-Host "==> Including latest.json for auto-update"
} elseif (-not $NotifyUpdate) {
  Write-Host "==> Skipping latest.json (no update notification)"
}

gh release upload $tag $uploadAssets --clobber
if ($LASTEXITCODE -ne 0) { throw "Failed to upload assets" }

gh release edit $tag --latest
if ($LASTEXITCODE -ne 0) { Write-Host "    Warning: could not mark release as latest" }

# ── Done ───────────────────────────────────────────────────────────────────────

Write-Host ""
Write-Host "╔══════════════════════════════════════════╗"
Write-Host "║  Released $productName $tag"
Write-Host "╚══════════════════════════════════════════╝"
Write-Host ""
Write-Host "  GitHub:   https://github.com/$GitHubRepo/releases/tag/$tag"
Write-Host "  Download: https://github.com/$GitHubRepo/releases/latest/download/${productName}_x64-setup.exe"
Write-Host ""
