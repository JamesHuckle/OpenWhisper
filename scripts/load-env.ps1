# Load .env file into process environment variables.
# Supports KEY=value and KEY="value" (expands \n to newlines).
# Usage: . "$PSScriptRoot/load-env.ps1" (dot-source from another script)

param(
  [string]$Path = (Join-Path (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)) ".env")
)

if (-not (Test-Path $Path)) {
  return
}

$content = Get-Content $Path -Raw
$lines = $content -split "`n"
$i = 0
while ($i -lt $lines.Count) {
  $line = $lines[$i]
  $i++
  $trimmed = $line.Trim()
  if (-not $trimmed -or $trimmed.StartsWith("#")) {
    continue
  }
  $eqIdx = $trimmed.IndexOf("=")
  if ($eqIdx -le 0) {
    continue
  }
  $key = $trimmed.Substring(0, $eqIdx).Trim()
  $valueRaw = $trimmed.Substring($eqIdx + 1)
  if ($valueRaw.StartsWith('"')) {
    $value = $valueRaw.Substring(1)
    while (-not $value.EndsWith('"') -and $i -lt $lines.Count) {
      $value += "`n" + $lines[$i]
      $i++
    }
    $value = $value.TrimEnd('"')
  } else {
    $value = $valueRaw.Trim()
  }
  $value = $value -replace '\\n', "`n" -replace '\\r', "`r"
  [Environment]::SetEnvironmentVariable($key, $value, "Process")
}
