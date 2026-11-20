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
  return /(?:^|[\s"'\\/])scripts[\\/]invoke-codex-agent\.mjs\b/i.test(command);
}

// Each command of a compound line (;, &&, ||, |, newline) is checked on its
// own, so naming the wrapper in one never lets another launch Codex directly.
function launchesCodexDirectly(command) {
  return stripQuotedRegions(command)
    .split(/[;&|\r\n]+/)
    .some(part => isDirectCodex(part) && !isCheckedWrapper(part));
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

    if (launchesCodexDirectly(command)) {
      respond({
        continue: true,
        permission: "deny",
        user_message: "Direct Codex execution is blocked in this project.",
        agent_message: "Use node scripts/invoke-codex-agent.mjs --task '<task>'; do not bypass the A2A wrapper."
      });
      return;
    }

    respond({ continue: true, permission: "allow" });
  } catch (error) {
    const raw = String(input || "");
    if (launchesCodexDirectly(raw)) {
      respond({
        continue: true,
        permission: "deny",
        user_message: "Direct Codex execution is blocked in this project.",
        agent_message: "Use node scripts/invoke-codex-agent.mjs --task '<task>'; do not bypass the A2A wrapper."
      });
      return;
    }

    respond({ continue: true, permission: "allow" });
  }
});
