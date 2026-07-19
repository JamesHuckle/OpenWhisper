param(
  [string]$Tag,
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$androidProject = Join-Path $repoRoot "apps\android"
$gradleFile = Join-Path $androidProject "app\build.gradle.kts"
$gradleText = Get-Content -LiteralPath $gradleFile -Raw
$versionMatch = [regex]::Match($gradleText, 'versionName\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
  throw "Unable to read Android versionName from $gradleFile"
}
$version = $versionMatch.Groups[1].Value
$versionCodeMatch = [regex]::Match($gradleText, 'versionCode\s*=\s*(\d+)')
if (-not $versionCodeMatch.Success) {
  throw "Unable to read Android versionCode from $gradleFile"
}
$versionCode = [int]$versionCodeMatch.Groups[1].Value
if (-not $Tag) {
  $Tag = "v$version"
}

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
  throw "GitHub CLI is required. Install it and run 'gh auth login'."
}
& gh auth status | Out-Null
if ($LASTEXITCODE -ne 0) {
  throw "GitHub CLI is not authenticated. Run 'gh auth login'."
}

& "$PSScriptRoot/build-android-release.ps1" -SkipBuild:$SkipBuild
if ($LASTEXITCODE -ne 0) {
  throw "Android release build failed."
}

$outputDirectory = Join-Path $androidProject "app\build\outputs\apk\public"
$versionedApk = Join-Path $outputDirectory "OpenWhisper-Android-v$version.apk"
$stableApk = Join-Path $outputDirectory "OpenWhisper-Android.apk"
$checksumPath = Join-Path $outputDirectory "OpenWhisper-Android-v$version.apk.sha256"
$updateManifestPath = Join-Path $outputDirectory "OpenWhisper-Android-update.json"

$releaseJson = & gh release view $Tag --json body,url 2>$null
if ($LASTEXITCODE -ne 0) {
  # GitHub's /releases/latest URLs are shared by both platforms. Preserve the
  # stable Windows installer and its signed updater feed when Android advances
  # independently, otherwise the website and existing desktop clients would
  # start receiving 404s as soon as this release becomes latest.
  $previousTag = (& gh release view --json tagName --jq .tagName).Trim()
  $carryForwardDirectory = Join-Path ([IO.Path]::GetTempPath()) "openwhisper-release-assets-$([guid]::NewGuid().ToString('N'))"
  New-Item -ItemType Directory -Path $carryForwardDirectory | Out-Null
  foreach ($assetName in @("OpenWhisper_x64-setup.exe", "latest.json")) {
    & gh release download $previousTag --pattern $assetName --dir $carryForwardDirectory
    if ($LASTEXITCODE -ne 0) {
      throw "Unable to carry $assetName forward from $previousTag. Refusing to break stable Windows downloads."
    }
  }
  Write-Host "==> Creating GitHub release $Tag"
  & gh release create $Tag --title "OpenWhisper $Tag" --generate-notes
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to create GitHub release $Tag."
  }
  $releaseJson = & gh release view $Tag --json body,url
}
$release = $releaseJson | ConvertFrom-Json
$carryForwardAssets = if ($carryForwardDirectory) {
  @(Get-ChildItem -LiteralPath $carryForwardDirectory -File | ForEach-Object FullName)
} else {
  @()
}

$repoName = (& gh repo view --json nameWithOwner --jq .nameWithOwner).Trim()
$downloadUrl = "https://github.com/$repoName/releases/download/$Tag/OpenWhisper-Android.apk"
$sha256 = (Get-FileHash -LiteralPath $stableApk -Algorithm SHA256).Hash.ToLowerInvariant()
$updateManifest = @{
  versionCode = $versionCode
  versionName = $version
  apkUrl = $downloadUrl
  sha256 = $sha256
  releaseNotes = ""
} | ConvertTo-Json -Compress
[IO.File]::WriteAllText(
  $updateManifestPath,
  $updateManifest,
  [Text.UTF8Encoding]::new($false)
)

Write-Host "==> Uploading Android APKs and update feed to $Tag"
$uploadAssets = @($versionedApk, $stableApk, $checksumPath, $updateManifestPath) + $carryForwardAssets
& gh release upload $Tag $uploadAssets --clobber
if ($LASTEXITCODE -ne 0) {
  throw "GitHub release upload failed."
}

& gh release edit $Tag --latest
if ($LASTEXITCODE -ne 0) {
  throw "Unable to mark $Tag as the latest release."
}
$beginMarker = "<!-- openwhisper-android-begin -->"
$endMarker = "<!-- openwhisper-android-end -->"
$androidNotes = @"
$beginMarker
## Android beta

Download **OpenWhisper-Android.apk** to install OpenWhisper on Android 9 or newer. The app works alongside the selected keyboard and requires microphone and user-enabled accessibility permissions. Live transcription uses a personal OpenAI API key saved in the app. On Samsung devices, Auto Blocker may need to be disabled temporarily for installation.

**[Download the Android APK]($downloadUrl)**

SHA-256 checksums are included in the release assets. This APK is signed with the persistent OpenWhisper Android release key so future versions can update it in place.
$endMarker
"@

$body = [string]$release.body
$managedPattern = [regex]::Escape($beginMarker) + ".*?" + [regex]::Escape($endMarker)
if ($body -match $managedPattern) {
  $body = [regex]::Replace($body, $managedPattern, $androidNotes, [Text.RegularExpressions.RegexOptions]::Singleline)
} elseif ([string]::IsNullOrWhiteSpace($body)) {
  $body = $androidNotes
} else {
  $body = $body.TrimEnd() + "`n`n" + $androidNotes
}

& gh release edit $Tag --notes $body
if ($LASTEXITCODE -ne 0) {
  throw "Unable to update GitHub release notes."
}

Write-Host "==> Verifying public Android download"
$temporaryApk = Join-Path ([IO.Path]::GetTempPath()) "OpenWhisper-Android-$([guid]::NewGuid().ToString('N')).apk"
$temporaryManifest = Join-Path ([IO.Path]::GetTempPath()) "OpenWhisper-Android-update-$([guid]::NewGuid().ToString('N')).json"
try {
  Invoke-WebRequest -Uri $downloadUrl -OutFile $temporaryApk -UseBasicParsing
  $localHash = (Get-FileHash -LiteralPath $stableApk -Algorithm SHA256).Hash
  $downloadHash = (Get-FileHash -LiteralPath $temporaryApk -Algorithm SHA256).Hash
  if ($localHash -ne $downloadHash) {
    throw "Public APK hash does not match the locally verified artifact."
  }
  & "$PSScriptRoot/test-android-release.ps1" -ApkPath $temporaryApk -ExpectedVersion $version
  if ($LASTEXITCODE -ne 0) {
    throw "Downloaded APK verification failed."
  }
  $manifestUrl = "https://github.com/$repoName/releases/download/$Tag/OpenWhisper-Android-update.json"
  Invoke-WebRequest -Uri $manifestUrl -OutFile $temporaryManifest -UseBasicParsing
  $publishedManifest = Get-Content -LiteralPath $temporaryManifest -Raw | ConvertFrom-Json
  if (
    [int]$publishedManifest.versionCode -ne $versionCode -or
    [string]$publishedManifest.versionName -ne $version -or
    [string]$publishedManifest.apkUrl -ne $downloadUrl -or
    [string]$publishedManifest.sha256 -ne $localHash.ToLowerInvariant()
  ) {
    throw "Published Android update manifest does not match the verified APK."
  }
} finally {
  if (Test-Path -LiteralPath $temporaryApk) {
    Remove-Item -LiteralPath $temporaryApk -Force
  }
  if (Test-Path -LiteralPath $temporaryManifest) {
    Remove-Item -LiteralPath $temporaryManifest -Force
  }
  if ($carryForwardDirectory -and (Test-Path -LiteralPath $carryForwardDirectory)) {
    Get-ChildItem -LiteralPath $carryForwardDirectory -File | Remove-Item -Force
    Remove-Item -LiteralPath $carryForwardDirectory -Force
  }
}

Write-Host "Android release published."
Write-Host "    Release:  $($release.url)"
Write-Host "    Download: $downloadUrl"
