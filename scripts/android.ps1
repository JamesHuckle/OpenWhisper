param(
  [Parameter(ValueFromRemainingArguments = $true)]
  [string[]]$GradleArguments = @("testDebugUnitTest")
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$jdkRoot = Join-Path $repoRoot ".tools\jdk-17"
$androidRoot = Join-Path $repoRoot ".tools\android-sdk"
$androidProject = Join-Path $repoRoot "apps\android"
$gradleWrapper = Join-Path $androidProject "gradlew.bat"

if (-not (Test-Path $gradleWrapper)) {
  throw "Android toolchain is missing. Run .\scripts\bootstrap-android.ps1 first."
}

$env:JAVA_HOME = $jdkRoot
$env:ANDROID_HOME = $androidRoot
$env:ANDROID_SDK_ROOT = $androidRoot
$env:Path = "$(Join-Path $jdkRoot 'bin');$(Join-Path $androidRoot 'platform-tools');$env:Path"

& $gradleWrapper -p $androidProject @GradleArguments
exit $LASTEXITCODE
