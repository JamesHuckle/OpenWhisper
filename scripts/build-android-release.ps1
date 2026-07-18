param(
  [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$envPath = Join-Path $repoRoot ".env"
$androidProject = Join-Path $repoRoot "apps\android"
$androidSdk = Join-Path $repoRoot ".tools\android-sdk"
$jdkRoot = Join-Path $repoRoot ".tools\jdk-17"

. "$PSScriptRoot/load-env.ps1" -Path $envPath

$requiredVariables = @(
  "OPENWHISPER_ANDROID_KEYSTORE",
  "OPENWHISPER_ANDROID_KEY_ALIAS",
  "OPENWHISPER_ANDROID_STORE_PASSWORD",
  "OPENWHISPER_ANDROID_KEY_PASSWORD"
)
$missingVariables = $requiredVariables | Where-Object {
  -not [Environment]::GetEnvironmentVariable($_, "Process")
}
if ($missingVariables) {
  throw "Android signing is not configured. Run .\scripts\setup-android-signing.ps1 first. Missing: $($missingVariables -join ', ')"
}
if (-not (Test-Path -LiteralPath $env:OPENWHISPER_ANDROID_KEYSTORE)) {
  throw "Android release keystore not found: $env:OPENWHISPER_ANDROID_KEYSTORE"
}

$gradleFile = Join-Path $androidProject "app\build.gradle.kts"
$gradleText = Get-Content -LiteralPath $gradleFile -Raw
$versionMatch = [regex]::Match($gradleText, 'versionName\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
  throw "Unable to read Android versionName from $gradleFile"
}
$version = $versionMatch.Groups[1].Value

if (-not $SkipBuild) {
  Write-Host "==> Building and linting Android release"
  & "$PSScriptRoot/android.ps1" --stop | Out-Null
  $buildSucceeded = $false
  for ($attempt = 1; $attempt -le 2; $attempt += 1) {
    # Gradle owns incremental invalidation. Avoid its clean task here because
    # OneDrive and antivirus scanners can transiently lock generated files.
    & "$PSScriptRoot/android.ps1" assembleRelease lintRelease
    if ($LASTEXITCODE -eq 0) {
      $buildSucceeded = $true
      break
    }
    if ($attempt -lt 2) {
      Write-Warning "Gradle build attempt $attempt failed; stopping the daemon and retrying once."
      & "$PSScriptRoot/android.ps1" --stop | Out-Null
      Start-Sleep -Seconds 1
    }
  }
  if (-not $buildSucceeded) {
    throw "Android release build failed after two attempts."
  }
}

$unsignedApk = Join-Path $androidProject "app\build\outputs\apk\release\app-release-unsigned.apk"
if (-not (Test-Path -LiteralPath $unsignedApk)) {
  throw "Unsigned release APK not found: $unsignedApk"
}

$buildTools = Get-ChildItem -LiteralPath (Join-Path $androidSdk "build-tools") -Directory |
  Sort-Object { [version]$_.Name } -Descending |
  Select-Object -First 1
if (-not $buildTools) {
  throw "Android build tools are missing. Run .\scripts\bootstrap-android.ps1 first."
}

$zipalign = Join-Path $buildTools.FullName "zipalign.exe"
$apkSigner = Join-Path $buildTools.FullName "apksigner.bat"
$outputDirectory = Join-Path $androidProject "app\build\outputs\apk\public"
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null

$alignedApk = Join-Path $outputDirectory "OpenWhisper-Android-aligned.apk"
$versionedApk = Join-Path $outputDirectory "OpenWhisper-Android-v$version.apk"
$stableApk = Join-Path $outputDirectory "OpenWhisper-Android.apk"
$checksumPath = Join-Path $outputDirectory "OpenWhisper-Android-v$version.apk.sha256"

foreach ($path in @($alignedApk, $versionedApk, $stableApk, $checksumPath)) {
  if (Test-Path -LiteralPath $path) {
    Remove-Item -LiteralPath $path -Force
  }
}

$env:JAVA_HOME = $jdkRoot
$env:Path = "$(Join-Path $jdkRoot 'bin');$env:Path"

Write-Host "==> Aligning Android release APK"
& $zipalign -P 16 -f 4 $unsignedApk $alignedApk
if ($LASTEXITCODE -ne 0) {
  throw "zipalign failed."
}

Write-Host "==> Signing Android release APK"
& $apkSigner sign `
  --ks $env:OPENWHISPER_ANDROID_KEYSTORE `
  --ks-key-alias $env:OPENWHISPER_ANDROID_KEY_ALIAS `
  --ks-pass env:OPENWHISPER_ANDROID_STORE_PASSWORD `
  --key-pass env:OPENWHISPER_ANDROID_KEY_PASSWORD `
  --out $versionedApk `
  $alignedApk
if ($LASTEXITCODE -ne 0) {
  throw "apksigner failed."
}

Copy-Item -LiteralPath $versionedApk -Destination $stableApk -Force
$sha256 = (Get-FileHash -LiteralPath $versionedApk -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText(
  $checksumPath,
  "$sha256  $(Split-Path -Leaf $versionedApk)`n",
  [Text.UTF8Encoding]::new($false)
)
Remove-Item -LiteralPath $alignedApk -Force

& "$PSScriptRoot/test-android-release.ps1" -ApkPath $versionedApk -ExpectedVersion $version
if ($LASTEXITCODE -ne 0) {
  throw "Signed Android APK verification failed."
}

Write-Host "Android release artifacts ready:"
Write-Host "    $versionedApk"
Write-Host "    $stableApk"
Write-Host "    $checksumPath"
