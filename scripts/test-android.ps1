param(
  [switch]$ResetEmulator
)

$ErrorActionPreference = "Stop"

& (Join-Path $PSScriptRoot "android-emulator.ps1") start -Reset:$ResetEmulator
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$adb = Join-Path $repoRoot ".tools\android-sdk\platform-tools\adb.exe"
& $adb uninstall com.openwhisper.android.debug.test 2>$null | Out-Null
& $adb uninstall com.openwhisper.android.debug 2>$null | Out-Null

& (Join-Path $PSScriptRoot "android.ps1") clean testDebugUnitTest lintDebug connectedDebugAndroidTest assembleDebug
exit $LASTEXITCODE
