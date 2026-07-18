param(
  [ValidateSet("start", "stop", "status")]
  [string]$Action = "start",
  [switch]$Reset,
  [switch]$Visible
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$jdkRoot = Join-Path $repoRoot ".tools\jdk-17"
$androidRoot = Join-Path $repoRoot ".tools\android-sdk"
$avdHome = Join-Path $repoRoot ".tools\android-avd"
$androidUserHome = Join-Path $repoRoot ".tools\android-user"
$adb = Join-Path $androidRoot "platform-tools\adb.exe"
$emulator = Join-Path $androidRoot "emulator\emulator.exe"
$avdManager = Join-Path $androidRoot "cmdline-tools\latest\bin\avdmanager.bat"
$avdName = "openwhisper_api36"

if (-not (Test-Path $emulator)) {
  throw "Android emulator is missing. Run .\scripts\bootstrap-android.ps1 -WithEmulator."
}

$env:JAVA_HOME = $jdkRoot
$env:ANDROID_HOME = $androidRoot
$env:ANDROID_SDK_ROOT = $androidRoot
$env:ANDROID_AVD_HOME = $avdHome
$env:ANDROID_USER_HOME = $androidUserHome
$env:Path = "$(Join-Path $jdkRoot 'bin');$(Join-Path $androidRoot 'platform-tools');$env:Path"

if ($Action -eq "status") {
  & $adb devices -l
  exit $LASTEXITCODE
}

if ($Action -eq "stop") {
  $serials = & $adb devices | Select-String '^emulator-\d+\s+device$' | ForEach-Object {
    ($_ -split '\s+')[0]
  }
  foreach ($serial in $serials) {
    & $adb -s $serial emu kill | Out-Null
  }
  exit 0
}

New-Item -ItemType Directory -Force -Path $avdHome, $androidUserHome | Out-Null
$configPath = Join-Path $avdHome "$avdName.avd\config.ini"
if (-not (Test-Path $configPath)) {
  "no" | & $avdManager create avd --force --name $avdName `
    --package "system-images;android-36;google_apis;x86_64" --device "pixel_9_pro_xl"
}

$existing = & $adb devices | Select-String '^emulator-\d+\s+device$'
if (-not $existing) {
  $arguments = @(
    "-avd", $avdName,
    "-no-audio",
    "-no-boot-anim",
    "-no-snapshot",
    "-gpu", "swiftshader_indirect"
  )
  if (-not $Visible) { $arguments += "-no-window" }
  if ($Reset) { $arguments += "-wipe-data" }
  $startOptions = @{
    FilePath = $emulator
    ArgumentList = $arguments
    PassThru = $true
  }
  if (-not $Visible) { $startOptions.WindowStyle = "Hidden" }
  Start-Process @startOptions | Out-Null
}

& $adb wait-for-device
$deadline = (Get-Date).AddMinutes(4)
do {
  Start-Sleep -Seconds 2
  $booted = (& $adb shell getprop sys.boot_completed 2>$null).Trim()
  if ((Get-Date) -gt $deadline) { throw "Android emulator did not boot within four minutes." }
} until ($booted -eq "1")

& $adb shell settings put secure show_ime_with_hard_keyboard 1 | Out-Null
& $adb shell input keyevent 82 | Out-Null
Write-Host "Android emulator is ready."
