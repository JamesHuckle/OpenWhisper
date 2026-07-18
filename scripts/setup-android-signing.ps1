param(
  [string]$KeyStorePath
)

$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$envPath = Join-Path $repoRoot ".env"
$jdkRoot = Join-Path $repoRoot ".tools\jdk-17"
$keytool = Join-Path $jdkRoot "bin\keytool.exe"

if (-not (Test-Path -LiteralPath $keytool)) {
  throw "Repository JDK is missing. Run .\scripts\bootstrap-android.ps1 first."
}

. "$PSScriptRoot/load-env.ps1" -Path $envPath

if (-not $KeyStorePath) {
  if ($env:OPENWHISPER_ANDROID_KEYSTORE) {
    $KeyStorePath = $env:OPENWHISPER_ANDROID_KEYSTORE
  } else {
    $KeyStorePath = Join-Path $env:USERPROFILE ".openwhisper\android\openwhisper-release.jks"
  }
}

$keyAlias = if ($env:OPENWHISPER_ANDROID_KEY_ALIAS) {
  $env:OPENWHISPER_ANDROID_KEY_ALIAS
} else {
  "openwhisper"
}

$alreadyConfigured =
  (Test-Path -LiteralPath $KeyStorePath) -and
  $env:OPENWHISPER_ANDROID_STORE_PASSWORD -and
  $env:OPENWHISPER_ANDROID_KEY_PASSWORD

if ($alreadyConfigured) {
  Write-Host "Android release signing is already configured."
  Write-Host "    Keystore: $KeyStorePath"
  exit 0
}

if (Test-Path -LiteralPath $KeyStorePath) {
  throw @"
The Android keystore already exists, but its credentials are not configured in .env:
  $KeyStorePath

Restore OPENWHISPER_ANDROID_STORE_PASSWORD and OPENWHISPER_ANDROID_KEY_PASSWORD.
The keystore must not be replaced or existing users will be unable to install updates.
"@
}

$keyDirectory = Split-Path -Parent $KeyStorePath
New-Item -ItemType Directory -Path $keyDirectory -Force | Out-Null

$randomBytes = New-Object byte[] 36
[Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($randomBytes)
$password = [Convert]::ToBase64String($randomBytes).TrimEnd("=").Replace("+", "-").Replace("/", "_")

$env:JAVA_HOME = $jdkRoot
$env:Path = "$(Join-Path $jdkRoot 'bin');$env:Path"
$env:OPENWHISPER_ANDROID_STORE_PASSWORD = $password
$env:OPENWHISPER_ANDROID_KEY_PASSWORD = $password
$env:OPENWHISPER_ANDROID_KEYSTORE = $KeyStorePath
$env:OPENWHISPER_ANDROID_KEY_ALIAS = $keyAlias

Write-Host "==> Generating long-lived Android release signing key"
& $keytool `
  -genkeypair `
  -noprompt `
  -keystore $KeyStorePath `
  -storetype JKS `
  -alias $keyAlias `
  -keyalg RSA `
  -keysize 4096 `
  -sigalg SHA256withRSA `
  -validity 10000 `
  -dname "CN=OpenWhisper, O=OpenWhisper, C=GB" `
  -storepass:env OPENWHISPER_ANDROID_STORE_PASSWORD `
  -keypass:env OPENWHISPER_ANDROID_KEY_PASSWORD
if ($LASTEXITCODE -ne 0) {
  throw "Android signing key generation failed."
}

$envBlock = @"

# Android release signing. Never commit these values.
OPENWHISPER_ANDROID_KEYSTORE=$KeyStorePath
OPENWHISPER_ANDROID_KEY_ALIAS=$keyAlias
OPENWHISPER_ANDROID_STORE_PASSWORD=$password
OPENWHISPER_ANDROID_KEY_PASSWORD=$password
"@
[IO.File]::AppendAllText($envPath, $envBlock, [Text.UTF8Encoding]::new($false))

Write-Host "Android release signing configured."
Write-Host "    Keystore: $KeyStorePath"
Write-Host "    Credentials: $envPath (ignored by Git)"
Write-Warning "Back up both files securely. Losing this keystore prevents updates to installed APKs."
