$ErrorActionPreference = "Stop"

try {
    $payload = [Console]::In.ReadToEnd() | ConvertFrom-Json
    $command = [string]$payload.command

    # Deny direct Codex CLI launches. The checked wrapper remains the sole entry
    # point, making recursion, locking, working-directory, and logging mandatory.
    $mentionsCodex = $command -match '(?i)(^|[;&|\s"''])(codex(?:\.exe)?)(?=\s|$|[;&|"''])' -or
        $command -match '(?i)\bnpx(?:\.cmd)?\s+[^\r\n]*\bcodex\b'
    $usesWrapper = $command -match '(?i)(?:^|[\\/])scripts[\\/]invoke-codex-agent\.ps1\b'

    if ($mentionsCodex -and -not $usesWrapper) {
        [pscustomobject]@{
            continue = $true
            permission = "deny"
            user_message = "Direct Codex execution is blocked in this project."
            agent_message = "Use .\\scripts\\invoke-codex-agent.ps1; do not bypass the A2A wrapper."
        } | ConvertTo-Json -Compress
        exit 0
    }

    [pscustomobject]@{ continue = $true; permission = "allow" } |
        ConvertTo-Json -Compress
} catch {
    [pscustomobject]@{
        continue = $true
        permission = "deny"
        user_message = "The project shell guard failed closed."
        agent_message = "Shell guard error: $($_.Exception.Message)"
    } | ConvertTo-Json -Compress
}

