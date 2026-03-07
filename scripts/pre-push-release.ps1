$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Split-Path -Parent $PSScriptRoot)).Path `
  -replace '^Microsoft\.PowerShell\.Core\\FileSystem::', ''

function Resolve-RequiredCommand {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Name,
    [string[]]$Candidates = @()
  )

  $command = Get-Command $Name -ErrorAction SilentlyContinue
  if ($command) {
    return $command.Source
  }

  foreach ($candidate in $Candidates) {
    if (-not [string]::IsNullOrWhiteSpace($candidate) -and (Test-Path $candidate)) {
      $dir = Split-Path -Parent $candidate
      $pathEntries = @($env:Path -split ';')
      if ($pathEntries -notcontains $dir) {
        $env:Path = "$dir;$env:Path"
      }
      return $candidate
    }
  }

  $candidateList = ($Candidates | Where-Object { -not [string]::IsNullOrWhiteSpace($_) }) -join ", "
  if ([string]::IsNullOrWhiteSpace($candidateList)) {
    throw "Required command '$Name' was not found in PATH."
  }

  throw "Required command '$Name' was not found in PATH or common install locations: $candidateList"
}

function Get-GitHubRepoInfo {
  $originUrl = (git remote get-url origin).Trim()
  if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($originUrl)) {
    throw "Failed to read git remote 'origin'."
  }

  if ($originUrl -match '^https://github\.com/(?<owner>[^/]+)/(?<repo>[^/.]+?)(?:\.git)?$') {
    return @{
      Owner = $matches.owner
      Repo = $matches.repo
    }
  }

  if ($originUrl -match '^git@github\.com:(?<owner>[^/]+)/(?<repo>[^/.]+?)(?:\.git)?$') {
    return @{
      Owner = $matches.owner
      Repo = $matches.repo
    }
  }

  throw "Unsupported GitHub origin URL: $originUrl"
}

$ghPath = Resolve-RequiredCommand -Name "gh.exe" -Candidates @(
  (Join-Path ${env:ProgramFiles} "GitHub CLI\gh.exe"),
  (Join-Path $env:LOCALAPPDATA "Programs\GitHub CLI\gh.exe")
)

$cargoPath = Resolve-RequiredCommand -Name "cargo.exe" -Candidates @(
  (Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe")
)

Write-Host "    gh: $ghPath"
Write-Host "    cargo: $cargoPath"

$null = & $ghPath auth status 2>&1
if ($LASTEXITCODE -ne 0) {
  throw "GitHub CLI is installed but not authenticated. Run 'gh auth login' and retry."
}

# Read version
$tauriConfPath = Join-Path $RepoRoot "apps/desktop/src-tauri/tauri.conf.json"
$tauriConf = Get-Content $tauriConfPath -Raw | ConvertFrom-Json
$version = $tauriConf.version
$productName = $tauriConf.productName
$tag = "v$version"

Write-Host ""
Write-Host "=========================================="
Write-Host "Building release: $productName $tag"
Write-Host "=========================================="
Write-Host ""

# Build the Windows installer
& "$PSScriptRoot/build-windows-installer.ps1"

# Locate installer
$CargoTargetDir = Join-Path $env:USERPROFILE "openwhisper-cargo-target"
$InstallerDir = Join-Path $CargoTargetDir "release/bundle/nsis"
$VersionedInstaller = Join-Path $InstallerDir "${productName}_${version}_x64-setup.exe"

if (-not (Test-Path $VersionedInstaller)) {
  throw "Installer not found: $VersionedInstaller"
}

$StableInstaller = Join-Path $InstallerDir "${productName}_x64-setup.exe"
Copy-Item -Path $VersionedInstaller -Destination $StableInstaller -Force

# Tag and push with recursion guard
Write-Host "==> Tagging $tag"
git tag -f $tag
if ($LASTEXITCODE -ne 0) {
  throw "Failed to create tag"
}

$env:OPENWHISPER_RELEASING = "1"
try {
  git push origin $tag --force
  if ($LASTEXITCODE -ne 0) {
    throw "Failed to push tag"
  }
}
finally {
  $env:OPENWHISPER_RELEASING = $null
}

# Create or update GitHub release
Write-Host "==> Publishing GitHub Release $tag"

$repoInfo = Get-GitHubRepoInfo
$ghToken = (& $ghPath auth token).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($ghToken)) {
  throw "Failed to retrieve GitHub auth token from gh."
}

$releaseHeaders = @{
  Authorization = "Bearer $ghToken"
  Accept = "application/vnd.github+json"
  "X-GitHub-Api-Version" = "2022-11-28"
  "User-Agent" = "OpenWhisper release hook"
}

$releaseLookupUrl = "https://api.github.com/repos/$($repoInfo.Owner)/$($repoInfo.Repo)/releases/tags/$tag"
$releaseExists = $false

try {
  Invoke-RestMethod -Method Get -Uri $releaseLookupUrl -Headers $releaseHeaders | Out-Null
  $releaseExists = $true
} catch {
  $statusCode = $null
  if ($_.Exception.Response -and $_.Exception.Response.StatusCode) {
    $statusCode = [int]$_.Exception.Response.StatusCode
  }

  if ($statusCode -ne 404) {
    throw
  }
}

if (-not $releaseExists) {
  & $ghPath release create $tag `
    --title "$productName $tag" `
    --generate-notes `
    --latest
  if ($LASTEXITCODE -ne 0) {
    throw "Failed to create release"
  }
}

& $ghPath release upload $tag $VersionedInstaller $StableInstaller --clobber
if ($LASTEXITCODE -ne 0) {
  throw "Failed to upload assets"
}

& $ghPath release edit $tag --latest
if ($LASTEXITCODE -ne 0) {
  Write-Host "Warning: could not mark release as latest"
}

# Done
Write-Host ""
Write-Host "=========================================="
Write-Host "Released $productName $tag"
Write-Host "=========================================="
Write-Host ""
Write-Host "GitHub:   https://github.com/JamesHuckle/OpenWhisper/releases/tag/$tag"
Write-Host ("Download: https://github.com/JamesHuckle/OpenWhisper/releases/latest/download/{0}_x64-setup.exe" -f $productName)
Write-Host ""
