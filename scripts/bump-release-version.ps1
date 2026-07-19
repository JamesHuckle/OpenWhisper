$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$tauriPath = Join-Path $repoRoot "apps\desktop\src-tauri\tauri.conf.json"
$cargoPath = Join-Path $repoRoot "apps\desktop\src-tauri\Cargo.toml"
$cargoLockPath = Join-Path $repoRoot "apps\desktop\src-tauri\Cargo.lock"
$packagePath = Join-Path $repoRoot "apps\desktop\package.json"
$packageLockPath = Join-Path $repoRoot "apps\desktop\package-lock.json"
$androidPath = Join-Path $repoRoot "apps\android\app\build.gradle.kts"

function Read-Match {
  param(
    [Parameter(Mandatory = $true)][string]$Text,
    [Parameter(Mandatory = $true)][string]$Pattern,
    [Parameter(Mandatory = $true)][string]$Description
  )

  $match = [regex]::Match($Text, $Pattern, [Text.RegularExpressions.RegexOptions]::Multiline)
  if (-not $match.Success) {
    throw "Unable to read $Description."
  }
  return $match.Groups[1].Value
}

function Read-HeadFile {
  param([Parameter(Mandatory = $true)][string]$Path)

  $content = & git -C $repoRoot show "HEAD:$Path" 2>$null
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to read $Path from HEAD."
  }
  return ($content -join "`n")
}

function Write-Utf8NoBom {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Text
  )

  [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}

$tauriText = Get-Content -LiteralPath $tauriPath -Raw
$cargoText = Get-Content -LiteralPath $cargoPath -Raw
$cargoLockText = Get-Content -LiteralPath $cargoLockPath -Raw
$packageText = Get-Content -LiteralPath $packagePath -Raw
$packageLockText = Get-Content -LiteralPath $packageLockPath -Raw
$androidText = Get-Content -LiteralPath $androidPath -Raw

$desktopVersionText = Read-Match $tauriText '"version"\s*:\s*"([^"]+)"' "desktop version"
$androidVersionText = Read-Match $androidText 'versionName\s*=\s*"([^"]+)"' "Android version"
$androidVersionCode = [int](Read-Match $androidText 'versionCode\s*=\s*(\d+)' "Android versionCode")
$headTauri = Read-HeadFile "apps/desktop/src-tauri/tauri.conf.json"
$headAndroid = Read-HeadFile "apps/android/app/build.gradle.kts"
$headDesktopVersionText = Read-Match $headTauri '"version"\s*:\s*"([^"]+)"' "desktop version in HEAD"
$headAndroidVersionText = Read-Match $headAndroid 'versionName\s*=\s*"([^"]+)"' "Android version in HEAD"
$headAndroidVersionCode = [int](Read-Match $headAndroid 'versionCode\s*=\s*(\d+)' "Android versionCode in HEAD")

$desktopVersion = [version]$desktopVersionText
$androidVersion = [version]$androidVersionText
$headDesktopVersion = [version]$headDesktopVersionText
$headAndroidVersion = [version]$headAndroidVersionText
$workingVersionWasBumped = $desktopVersion -ne $headDesktopVersion -or $androidVersion -ne $headAndroidVersion

if ($workingVersionWasBumped) {
  $targetVersion = if ($desktopVersion -gt $androidVersion) { $desktopVersion } else { $androidVersion }
} else {
  $currentVersion = if ($desktopVersion -gt $androidVersion) { $desktopVersion } else { $androidVersion }
  $targetVersion = [version]::new($currentVersion.Major, $currentVersion.Minor, $currentVersion.Build + 1)
}

$targetVersionText = "$($targetVersion.Major).$($targetVersion.Minor).$($targetVersion.Build)"
$targetVersionCode = if (
  $workingVersionWasBumped -and
  $androidVersion -eq $targetVersion -and
  $androidVersionCode -gt $headAndroidVersionCode
) {
  $androidVersionCode
} else {
  [Math]::Max($androidVersionCode, $headAndroidVersionCode) + 1
}

$tauriText = [regex]::Replace(
  $tauriText,
  '("version"\s*:\s*")[^"]+("\s*,)',
  "`${1}$targetVersionText`${2}",
  1
)
$cargoText = [regex]::Replace(
  $cargoText,
  '(?m)^(version\s*=\s*")[^"]+("\s*)$',
  "`${1}$targetVersionText`${2}",
  1
)
$cargoLockText = [regex]::Replace(
  $cargoLockText,
  '(?ms)(\[\[package\]\]\s+name\s*=\s*"openwhisper_desktop"\s+version\s*=\s*")[^"]+("\s*)',
  "`${1}$targetVersionText`${2}",
  1
)
$packageText = [regex]::Replace(
  $packageText,
  '("version"\s*:\s*")[^"]+("\s*,)',
  "`${1}$targetVersionText`${2}",
  1
)
$packageLockText = [regex]::new(
  '(?m)^(\s*"version"\s*:\s*")[^"]+("\s*,)'
).Replace($packageLockText, "`${1}$targetVersionText`${2}", 2)
$androidText = [regex]::Replace($androidText, '(versionCode\s*=\s*)\d+', "`${1}$targetVersionCode", 1)
$androidText = [regex]::Replace($androidText, '(versionName\s*=\s*")[^"]+("\s*)', "`${1}$targetVersionText`${2}", 1)

Write-Utf8NoBom $tauriPath $tauriText
Write-Utf8NoBom $cargoPath $cargoText
Write-Utf8NoBom $cargoLockPath $cargoLockText
Write-Utf8NoBom $packagePath $packageText
Write-Utf8NoBom $packageLockPath $packageLockText
Write-Utf8NoBom $androidPath $androidText

& git -C $repoRoot add -- `
  "apps/desktop/src-tauri/tauri.conf.json" `
  "apps/desktop/src-tauri/Cargo.toml" `
  "apps/desktop/src-tauri/Cargo.lock" `
  "apps/desktop/package.json" `
  "apps/desktop/package-lock.json" `
  "apps/android/app/build.gradle.kts"
if ($LASTEXITCODE -ne 0) {
  throw "Unable to stage synchronized release versions."
}

Write-Host "Release version synchronized at $targetVersionText (Android versionCode $targetVersionCode)."
