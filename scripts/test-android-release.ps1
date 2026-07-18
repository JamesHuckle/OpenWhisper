param(
  [Parameter(Mandatory = $true)]
  [string]$ApkPath,

  [string]$ExpectedVersion = "0.1.0",

  [switch]$InstallOnConnectedDevice
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$androidSdk = Join-Path $repoRoot ".tools\android-sdk"
$jdkRoot = Join-Path $repoRoot ".tools\jdk-17"
$resolvedApk = (Resolve-Path $ApkPath).Path

if (-not (Test-Path -LiteralPath (Join-Path $jdkRoot "bin\java.exe"))) {
  throw "Repository JDK is missing. Run .\scripts\bootstrap-android.ps1 first."
}
$env:JAVA_HOME = $jdkRoot
$env:Path = "$(Join-Path $jdkRoot 'bin');$env:Path"

$buildTools = Get-ChildItem -LiteralPath (Join-Path $androidSdk "build-tools") -Directory |
  Sort-Object { [version]$_.Name } -Descending |
  Select-Object -First 1
if (-not $buildTools) {
  throw "Android build tools are missing. Run .\scripts\bootstrap-android.ps1 first."
}

$apkSigner = Join-Path $buildTools.FullName "apksigner.bat"
$aapt = Join-Path $buildTools.FullName "aapt.exe"
$adb = Join-Path $androidSdk "platform-tools\adb.exe"

Write-Host "==> Verifying Android release signature"
$signatureReport = & $apkSigner verify --verbose --print-certs $resolvedApk 2>&1
if ($LASTEXITCODE -ne 0) {
  throw "APK signature verification failed:`n$($signatureReport -join "`n")"
}
$signatureText = $signatureReport -join "`n"
if (
  $signatureText -notmatch "Verified using v2 scheme \(APK Signature Scheme v2\): true" -and
  $signatureText -notmatch "Verified using v3 scheme \(APK Signature Scheme v3\): true"
) {
  throw "APK is missing a verified modern Android signature (v2 or v3)."
}

Write-Host "==> Verifying release manifest"
$badging = & $aapt dump badging $resolvedApk 2>&1
if ($LASTEXITCODE -ne 0) {
  throw "Unable to inspect APK manifest:`n$($badging -join "`n")"
}
$packageLine = $badging | Where-Object { $_ -like "package: name=*" } | Select-Object -First 1
if ($packageLine -notmatch "name='com\.openwhisper\.android'") {
  throw "Unexpected Android package identity: $packageLine"
}
if ($packageLine -notmatch "versionName='$([regex]::Escape($ExpectedVersion))'") {
  throw "Unexpected Android version: $packageLine"
}
if ($badging -contains "application-debuggable") {
  throw "Public APK must not be debuggable."
}

$sha256 = (Get-FileHash -LiteralPath $resolvedApk -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host "    Package: com.openwhisper.android"
Write-Host "    Version: $ExpectedVersion"
Write-Host "    SHA-256: $sha256"

if ($InstallOnConnectedDevice) {
  Write-Host "==> Installing release APK on connected Android device"
  & $adb install -r $resolvedApk
  if ($LASTEXITCODE -ne 0) {
    throw "adb install failed."
  }
  $packageReport = & $adb shell dumpsys package com.openwhisper.android
  if (($packageReport -join "`n") -notmatch "versionName=$([regex]::Escape($ExpectedVersion))") {
    throw "Installed package does not report version $ExpectedVersion."
  }
}

Write-Host "Android release APK verification passed."
