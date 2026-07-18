param(
  [switch]$WithEmulator
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$toolsRoot = Join-Path $repoRoot ".tools"
$downloads = Join-Path $toolsRoot "downloads"
$jdkRoot = Join-Path $toolsRoot "jdk-17"
$androidRoot = Join-Path $toolsRoot "android-sdk"
$cmdlineRoot = Join-Path $androidRoot "cmdline-tools\latest"
$gradleRoot = Join-Path $toolsRoot "gradle-9.5.0"

New-Item -ItemType Directory -Force -Path $downloads | Out-Null

if (-not (Test-Path (Join-Path $jdkRoot "bin\java.exe"))) {
  $jdkZip = Join-Path $downloads "temurin-jdk17.zip"
  if (-not (Test-Path $jdkZip)) {
    curl.exe -fL "https://api.adoptium.net/v3/binary/latest/17/ga/windows/x64/jdk/hotspot/normal/eclipse" -o $jdkZip
  }
  $jdkExtract = Join-Path $toolsRoot "jdk-extract"
  if (Test-Path $jdkExtract) { Remove-Item -Recurse -Force -LiteralPath $jdkExtract }
  Expand-Archive -Path $jdkZip -DestinationPath $jdkExtract
  $jdkDirectory = Get-ChildItem $jdkExtract -Directory | Select-Object -First 1
  Move-Item -LiteralPath $jdkDirectory.FullName -Destination $jdkRoot
  Remove-Item -Recurse -Force -LiteralPath $jdkExtract
}

$env:JAVA_HOME = $jdkRoot
$env:ANDROID_HOME = $androidRoot
$env:ANDROID_SDK_ROOT = $androidRoot
$env:Path = "$(Join-Path $jdkRoot 'bin');$(Join-Path $androidRoot 'platform-tools');$env:Path"

if (-not (Test-Path (Join-Path $cmdlineRoot "bin\sdkmanager.bat"))) {
  $commandLineZip = Join-Path $downloads "android-commandlinetools.zip"
  if (-not (Test-Path $commandLineZip)) {
    curl.exe -fL "https://dl.google.com/android/repository/commandlinetools-win-14742923_latest.zip" -o $commandLineZip
  }
  $commandLineExtract = Join-Path $toolsRoot "android-commandline-extract"
  if (Test-Path $commandLineExtract) { Remove-Item -Recurse -Force -LiteralPath $commandLineExtract }
  Expand-Archive -Path $commandLineZip -DestinationPath $commandLineExtract
  New-Item -ItemType Directory -Force -Path (Split-Path $cmdlineRoot) | Out-Null
  Move-Item -LiteralPath (Join-Path $commandLineExtract "cmdline-tools") -Destination $cmdlineRoot
  Remove-Item -Recurse -Force -LiteralPath $commandLineExtract
}

$sdkManager = Join-Path $cmdlineRoot "bin\sdkmanager.bat"
$licenseAnswers = 1..20 | ForEach-Object { "y" }
$licenseAnswers | & $sdkManager --sdk_root=$androidRoot --licenses | Out-Null
& $sdkManager --sdk_root=$androidRoot "platform-tools" "platforms;android-36" "build-tools;36.0.0"

if ($WithEmulator) {
  & $sdkManager --sdk_root=$androidRoot "emulator" "system-images;android-36;google_apis;x86_64"
}

if (-not (Test-Path (Join-Path $gradleRoot "bin\gradle.bat"))) {
  $gradleZip = Join-Path $downloads "gradle-9.5.0-bin.zip"
  if (-not (Test-Path $gradleZip)) {
    curl.exe -fL "https://services.gradle.org/distributions/gradle-9.5.0-bin.zip" -o $gradleZip
  }
  Expand-Archive -Path $gradleZip -DestinationPath $toolsRoot
}

$androidProject = Join-Path $repoRoot "apps\android"
& (Join-Path $gradleRoot "bin\gradle.bat") -p $androidProject wrapper --gradle-version 9.5.0

Write-Host "Android toolchain ready."
Write-Host "Run: .\scripts\android.ps1 testDebugUnitTest"
