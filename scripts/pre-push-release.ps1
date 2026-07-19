$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Split-Path -Parent $PSScriptRoot)).Path `
  -replace '^Microsoft\.PowerShell\.Core\\FileSystem::', ''

. "$PSScriptRoot/load-env.ps1" -Path (Join-Path $RepoRoot ".env")

function Resolve-RequiredCommand {
  param(
    [Parameter(Mandatory = $true)][string]$Name,
    [string[]]$Candidates = @()
  )

  $command = Get-Command $Name -ErrorAction SilentlyContinue
  if ($command) { return $command.Source }

  foreach ($candidate in $Candidates) {
    if (-not [string]::IsNullOrWhiteSpace($candidate) -and (Test-Path $candidate)) {
      $directory = Split-Path -Parent $candidate
      if (@($env:Path -split ';') -notcontains $directory) {
        $env:Path = "$directory;$env:Path"
      }
      return $candidate
    }
  }

  throw "Required command '$Name' was not found."
}

function Assert-LastExitCode {
  param([Parameter(Mandatory = $true)][string]$Message)
  if ($LASTEXITCODE -ne 0) { throw $Message }
}

function Get-GitHubRepoInfo {
  $originUrl = (git -C $RepoRoot remote get-url origin).Trim()
  Assert-LastExitCode "Failed to read git remote 'origin'."
  if ($originUrl -match '^https://github\.com/(?<owner>[^/]+)/(?<repo>[^/.]+?)(?:\.git)?$' -or
      $originUrl -match '^git@github\.com:(?<owner>[^/]+)/(?<repo>[^/.]+?)(?:\.git)?$') {
    return @{ Owner = $matches.owner; Repo = $matches.repo }
  }
  throw "Unsupported GitHub origin URL: $originUrl"
}

function Get-JsonWithRetry {
  param(
    [Parameter(Mandatory = $true)][string]$Uri,
    [int]$Attempts = 6
  )

  for ($attempt = 1; $attempt -le $Attempts; $attempt += 1) {
    try {
      return Invoke-RestMethod -Uri $Uri
    } catch {
      if ($attempt -eq $Attempts) { throw }
      Start-Sleep -Seconds 2
    }
  }
}

$workingTreeStatus = @(git -C $RepoRoot status --porcelain)
if ($workingTreeStatus.Count -gt 0) {
  throw "The release push requires a clean working tree. Commit every change first."
}

$ghPath = Resolve-RequiredCommand -Name "gh.exe" -Candidates @(
  (Join-Path ${env:ProgramFiles} "GitHub CLI\gh.exe"),
  (Join-Path $env:LOCALAPPDATA "Programs\GitHub CLI\gh.exe")
)
$null = Resolve-RequiredCommand -Name "cargo.exe" -Candidates @(
  (Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe")
)
$null = Resolve-RequiredCommand -Name "uv.exe"
$null = Resolve-RequiredCommand -Name "npm.cmd"

$null = & $ghPath auth status 2>&1
Assert-LastExitCode "GitHub CLI is installed but not authenticated. Run 'gh auth login' and retry."

$tauriConfPath = Join-Path $RepoRoot "apps/desktop/src-tauri/tauri.conf.json"
$androidGradlePath = Join-Path $RepoRoot "apps/android/app/build.gradle.kts"
$tauriConf = Get-Content $tauriConfPath -Raw | ConvertFrom-Json
$androidGradle = Get-Content $androidGradlePath -Raw
$androidVersionMatch = [regex]::Match($androidGradle, 'versionName\s*=\s*"([^"]+)"')
if (-not $androidVersionMatch.Success) { throw "Unable to read the Android version." }
$version = [string]$tauriConf.version
$androidVersion = $androidVersionMatch.Groups[1].Value
if ($androidVersion -ne $version) {
  throw "Desktop version $version and Android version $androidVersion differ. Commit normally so the pre-commit hook can synchronize them."
}

$productName = [string]$tauriConf.productName
$tag = "v$version"
$repoInfo = Get-GitHubRepoInfo
$currentCommit = (git -C $RepoRoot rev-parse HEAD).Trim()
$remoteTagCommit = & git -C $RepoRoot ls-remote origin "refs/tags/$tag" |
  ForEach-Object { ($_ -split "`t")[0] } |
  Select-Object -First 1
$remoteTagCommit = if ($null -eq $remoteTagCommit) { "" } else { [string]$remoteTagCommit.Trim() }
if ($remoteTagCommit -and $remoteTagCommit -ne $currentCommit) {
  throw "Release tag $tag already points to another commit. Commit normally to generate a newer version."
}

$keyPath = Join-Path $env:USERPROFILE ".tauri\openwhisper.key"
if (-not $env:TAURI_SIGNING_PRIVATE_KEY -and (Test-Path $keyPath)) {
  $env:TAURI_SIGNING_PRIVATE_KEY = Get-Content $keyPath -Raw
}
if (-not $env:TAURI_SIGNING_PRIVATE_KEY) {
  throw "Updater signing key required. Run .\scripts\setup-updater-keys.ps1"
}

Write-Host ""
Write-Host "============================================================"
Write-Host "Coordinated production release: $productName $tag"
Write-Host "Web + Windows desktop + Android"
Write-Host "============================================================"

Write-Host "==> Testing and building web"
$webSourceDirectory = Join-Path $RepoRoot "apps/web"
$webBuildDirectory = Join-Path $env:USERPROFILE "openwhisper-web-release-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $webBuildDirectory | Out-Null
try {
  & robocopy.exe $webSourceDirectory $webBuildDirectory /E /R:2 /W:1 /NFL /NDL /NJH /NJS /NP /XD node_modules .next | Out-Null
  if ($LASTEXITCODE -gt 7) { throw "Unable to prepare the local web build directory." }
  Push-Location $webBuildDirectory
  try {
    & npm.cmd ci
    Assert-LastExitCode "Web dependency installation failed."
    & npm.cmd test
    Assert-LastExitCode "Web tests failed."
    & npm.cmd run build
    Assert-LastExitCode "Web production build failed."
  } finally { Pop-Location }
} finally {
  $resolvedWebBuildDirectory = [IO.Path]::GetFullPath($webBuildDirectory)
  $resolvedUserProfile = [IO.Path]::GetFullPath($env:USERPROFILE).TrimEnd('\') + '\'
  if (
    $resolvedWebBuildDirectory.StartsWith($resolvedUserProfile, [StringComparison]::OrdinalIgnoreCase) -and
    (Split-Path -Leaf $resolvedWebBuildDirectory) -like "openwhisper-web-release-*" -and
    (Test-Path -LiteralPath $resolvedWebBuildDirectory)
  ) {
    Remove-Item -LiteralPath $resolvedWebBuildDirectory -Recurse -Force
  }
}

Write-Host "==> Testing worker"
$workerEnvironment = Join-Path $env:USERPROFILE "openwhisper-worker-env-$([guid]::NewGuid().ToString('N'))"
$previousWorkerEnvironment = $env:UV_PROJECT_ENVIRONMENT
Push-Location (Join-Path $RepoRoot "apps/worker")
try {
  $env:UV_PROJECT_ENVIRONMENT = $workerEnvironment
  & uv.exe sync --link-mode copy --reinstall
  Assert-LastExitCode "Worker dependency synchronization failed."
  & uv.exe run python -m unittest discover -s tests
  Assert-LastExitCode "Worker tests failed."
} finally {
  Pop-Location
  $env:UV_PROJECT_ENVIRONMENT = $previousWorkerEnvironment
  $resolvedWorkerEnvironment = [IO.Path]::GetFullPath($workerEnvironment)
  $resolvedUserProfile = [IO.Path]::GetFullPath($env:USERPROFILE).TrimEnd('\') + '\'
  if (
    $resolvedWorkerEnvironment.StartsWith($resolvedUserProfile, [StringComparison]::OrdinalIgnoreCase) -and
    (Split-Path -Leaf $resolvedWorkerEnvironment) -like "openwhisper-worker-env-*" -and
    (Test-Path -LiteralPath $resolvedWorkerEnvironment)
  ) {
    Remove-Item -LiteralPath $resolvedWorkerEnvironment -Recurse -Force
  }
}

Write-Host "==> Testing and building signed Android release"
& "$PSScriptRoot/android.ps1" testDebugUnitTest
Assert-LastExitCode "Android unit tests failed."
& "$PSScriptRoot/build-android-release.ps1"
Assert-LastExitCode "Android release build failed."

Write-Host "==> Building signed Windows release"
& "$PSScriptRoot/build-windows-installer.ps1"
Assert-LastExitCode "Windows release build failed."

Write-Host "==> Testing desktop"
Push-Location (Join-Path $RepoRoot "apps/desktop")
try {
  & npm.cmd test
  Assert-LastExitCode "Desktop tests failed."
} finally { Pop-Location }

$cargoTargetDir = Join-Path $env:USERPROFILE "openwhisper-cargo-target"
$installerDir = Join-Path $cargoTargetDir "release/bundle/nsis"
$versionedInstaller = Join-Path $installerDir "${productName}_${version}_x64-setup.exe"
$signaturePath = "$versionedInstaller.sig"
if (-not (Test-Path $versionedInstaller)) { throw "Installer not found: $versionedInstaller" }
if (-not (Test-Path $signaturePath)) { throw "Signed updater artifact not found: $signaturePath" }
$stableInstaller = Join-Path $installerDir "${productName}_x64-setup.exe"
Copy-Item $versionedInstaller $stableInstaller -Force

Write-Host "==> Publishing coordinated GitHub release $tag"
& git -C $RepoRoot tag -f $tag
Assert-LastExitCode "Failed to create release tag."
$env:OPENWHISPER_RELEASING = "1"
try {
  & git -C $RepoRoot push origin $tag --force
  Assert-LastExitCode "Failed to push release tag."
} finally { $env:OPENWHISPER_RELEASING = $null }

$releaseLookupUri = "https://api.github.com/repos/$($repoInfo.Owner)/$($repoInfo.Repo)/releases/tags/$tag"
$releaseExists = $false
try {
  Invoke-RestMethod -Uri $releaseLookupUri -Headers @{ "User-Agent" = "OpenWhisper release hook" } | Out-Null
  $releaseExists = $true
} catch {
  $statusCode = if ($_.Exception.Response) { [int]$_.Exception.Response.StatusCode } else { $null }
  if ($statusCode -ne 404) { throw }
}
if (-not $releaseExists) {
  & $ghPath release create $tag --title "$productName $tag" --generate-notes --latest
  Assert-LastExitCode "Failed to create GitHub release."
}

$pubDate = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
$signature = (Get-Content $signaturePath -Raw).Trim()
$installerUrl = "https://github.com/$($repoInfo.Owner)/$($repoInfo.Repo)/releases/download/$tag/${productName}_${version}_x64-setup.exe"
$temporaryReleaseDirectory = Join-Path $env:TEMP "openwhisper-release-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $temporaryReleaseDirectory | Out-Null
$latestJsonPath = Join-Path $temporaryReleaseDirectory "latest.json"
$latestJson = @{
  version = $version
  notes = "OpenWhisper $version is ready. Your settings and API key will be preserved."
  pub_date = $pubDate
  platforms = @{
    "windows-x86_64" = @{ signature = $signature; url = $installerUrl }
  }
} | ConvertTo-Json -Depth 5 -Compress
[IO.File]::WriteAllText($latestJsonPath, $latestJson, [Text.UTF8Encoding]::new($false))
try {
  & $ghPath release upload $tag $versionedInstaller $stableInstaller $latestJsonPath --clobber
  Assert-LastExitCode "Failed to upload Windows release assets."
  & $ghPath release edit $tag --latest
  Assert-LastExitCode "Failed to mark the coordinated release as latest."

  & "$PSScriptRoot/release-android.ps1" -Tag $tag -SkipBuild
  Assert-LastExitCode "Failed to publish Android release assets."

  $windowsFeed = Get-JsonWithRetry -Uri "https://github.com/$($repoInfo.Owner)/$($repoInfo.Repo)/releases/latest/download/latest.json"
  if ([string]$windowsFeed.version -ne $version) {
    throw "Published Windows update feed reports $($windowsFeed.version), expected $version."
  }
  $androidFeed = Get-JsonWithRetry -Uri "https://github.com/$($repoInfo.Owner)/$($repoInfo.Repo)/releases/latest/download/OpenWhisper-Android-update.json"
  if ([string]$androidFeed.versionName -ne $version) {
    throw "Published Android update feed reports $($androidFeed.versionName), expected $version."
  }
} finally {
  if (Test-Path $temporaryReleaseDirectory) {
    Remove-Item $temporaryReleaseDirectory -Recurse -Force
  }
}

Write-Host ""
Write-Host "All signed release artifacts and update notifications are live for $tag."
Write-Host "The main push will now complete and Vercel will deploy apps/web."
