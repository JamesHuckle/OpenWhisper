param(
  [Parameter(Mandatory = $true)][string]$InstallerPath,
  [Parameter(Mandatory = $true)][string]$ExpectedVersion
)

$ErrorActionPreference = "Stop"

$resolvedInstaller = (Resolve-Path -LiteralPath $InstallerPath).Path
$installer = Get-Item -LiteralPath $resolvedInstaller
$installerVersion = [string]$installer.VersionInfo.ProductVersion
if ($installerVersion -ne $ExpectedVersion) {
  throw "Installer version $installerVersion does not match expected version $ExpectedVersion."
}

$localAppData = [IO.Path]::GetFullPath($env:LOCALAPPDATA).TrimEnd('\')
$installRoot = [IO.Path]::GetFullPath((Join-Path $localAppData "OpenWhisper")).TrimEnd('\')
if (
  -not $installRoot.StartsWith("$localAppData\", [StringComparison]::OrdinalIgnoreCase) -or
  (Split-Path -Leaf $installRoot) -ne "OpenWhisper"
) {
  throw "Refusing to update unexpected installation directory: $installRoot"
}

$installedExecutable = Join-Path $installRoot "openwhisper_desktop.exe"
$mutex = [Threading.Mutex]::new($false, "Local\OpenWhisperLocalReleaseInstall")
$hasMutex = $false
try {
  $hasMutex = $mutex.WaitOne([TimeSpan]::FromMinutes(10))
  if (-not $hasMutex) {
    throw "Timed out waiting for another local OpenWhisper installation to finish."
  }

  $installedVersion = if (Test-Path -LiteralPath $installedExecutable) {
    [string](Get-Item -LiteralPath $installedExecutable).VersionInfo.ProductVersion
  } else {
    $null
  }
  if ($installedVersion -eq $ExpectedVersion) {
    Write-Host "    Local OpenWhisper is already current at $ExpectedVersion."
    return
  }

  $installRootPrefix = "$installRoot\"
  $localProcesses = @(Get-CimInstance Win32_Process | Where-Object {
    if ([string]::IsNullOrWhiteSpace($_.ExecutablePath)) { return $false }
    try {
      $processPath = [IO.Path]::GetFullPath($_.ExecutablePath)
      return $processPath.StartsWith($installRootPrefix, [StringComparison]::OrdinalIgnoreCase)
    } catch {
      return $false
    }
  })
  $restartApp = @($localProcesses | Where-Object { $_.Name -eq "openwhisper_desktop.exe" }).Count -gt 0

  foreach ($process in $localProcesses) {
    Stop-Process -Id $process.ProcessId -Force -ErrorAction Stop
  }

  $installProcess = Start-Process -FilePath $resolvedInstaller -ArgumentList "/S" -Wait -PassThru
  if ($installProcess.ExitCode -ne 0) {
    throw "OpenWhisper installer exited with code $($installProcess.ExitCode)."
  }
  if (-not (Test-Path -LiteralPath $installedExecutable)) {
    throw "Installer completed but the local OpenWhisper executable was not found."
  }

  $installedVersion = [string](Get-Item -LiteralPath $installedExecutable).VersionInfo.ProductVersion
  if ($installedVersion -ne $ExpectedVersion) {
    throw "Local OpenWhisper reports version $installedVersion after installing $ExpectedVersion."
  }

  if ($restartApp) {
    Start-Process -FilePath $installedExecutable
  }

  Write-Host "    Local OpenWhisper updated to $installedVersion."
  if ($restartApp) {
    Write-Host "    OpenWhisper was running and has been restarted."
  }
} finally {
  if ($hasMutex) { $mutex.ReleaseMutex() }
  $mutex.Dispose()
}
