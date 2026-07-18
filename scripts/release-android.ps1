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

$releaseJson = & gh release view $Tag --json body,url 2>$null
if ($LASTEXITCODE -ne 0) {
  Write-Host "==> Creating GitHub release $Tag"
  & gh release create $Tag --title "OpenWhisper $Tag" --generate-notes
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to create GitHub release $Tag."
  }
  $releaseJson = & gh release view $Tag --json body,url
}
$release = $releaseJson | ConvertFrom-Json

Write-Host "==> Uploading Android APKs to $Tag"
& gh release upload $Tag $versionedApk $stableApk $checksumPath --clobber
if ($LASTEXITCODE -ne 0) {
  throw "GitHub release upload failed."
}

$repoName = (& gh repo view --json nameWithOwner --jq .nameWithOwner).Trim()
$downloadUrl = "https://github.com/$repoName/releases/download/$Tag/OpenWhisper-Android.apk"
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
} finally {
  if (Test-Path -LiteralPath $temporaryApk) {
    Remove-Item -LiteralPath $temporaryApk -Force
  }
}

Write-Host "Android release published."
Write-Host "    Release:  $($release.url)"
Write-Host "    Download: $downloadUrl"
