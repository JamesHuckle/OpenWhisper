"use strict";

// Cross-platform Cursor hook: works in native Windows and WSL/Remote windows.
function respond(result) {
  process.stdout.write(JSON.stringify(result));
}

function stripQuotedRegions(command) {
  // Ignore quoted / here-string payloads so commit messages and task text that
  // mention "codex" do not look like CLI launches.
  return String(command)
    .replace(/@"[\s\S]*?"@/g, '""')
    .replace(/@'[\s\S]*?'@/g, "''")
    .replace(/<<[-]?['"]?\w+['"]?[\s\S]*?\n\w+\n/g, '""')
    .replace(/"(?:\\.|[^"\\])*"/g, '""')
    .replace(/'(?:\\.|[^'\\])*'/g, "''")
    .replace(/`(?:\\.|[^`\\])*`/g, "``");
}

function isDirectCodex(command) {
  const bare = stripQuotedRegions(command);
  return /(^|[;&|\s"'])(codex(?:\.exe)?)(?=\s|$|[;&|"'])/i.test(bare) ||
    /\bnpx(?:\.cmd)?\s+[^\r\n]*\bcodex\b/i.test(bare);
}

function isCheckedWrapper(command) {
  return /(?:^|[\\/])scripts[\\/]invoke-codex-agent\.(?:ps1|sh)\b/i.test(command);
}

let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", chunk => { input += chunk; });
process.stdin.on("end", () => {
  try {
    const trimmed = input.trim();
    if (!trimmed) {
      respond({ continue: true, permission: "allow" });
      return;
    }

    const payload = JSON.parse(trimmed);
    const command = String(payload.command || "");

    if (isDirectCodex(command) && !isCheckedWrapper(command)) {
      respond({
        continue: true,
        permission: "deny",
        user_message: "Direct Codex execution is blocked in this project.",
        agent_message: "Use scripts/invoke-codex-agent.ps1 on Windows or scripts/invoke-codex-agent.sh in WSL; do not bypass the A2A wrapper."
      });
      return;
    }

    respond({ continue: true, permission: "allow" });
  } catch (error) {
    const raw = String(input || "");
    if (isDirectCodex(raw) && !isCheckedWrapper(raw)) {
      respond({
        continue: true,
        permission: "deny",
        user_message: "Direct Codex execution is blocked in this project.",
        agent_message: "Use scripts/invoke-codex-agent.ps1 on Windows or scripts/invoke-codex-agent.sh in WSL; do not bypass the A2A wrapper."
      });
      return;
    }

    respond({ continue: true, permission: "allow" });
  }
});
