#!/usr/bin/env node
"use strict";

import { existsSync, readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const args = new Set(process.argv.slice(2));
if (args.has("--help") || args.has("-h")) {
  console.log(`Usage: node scripts/setup-cursor-codex-a2a.mjs [options]

Options:
  --install      Install/update @openai/codex globally with npm
  --login        Run the interactive ChatGPT login flow
  --smoke-test   Run a minimal read-only delegated Codex request
  --copy-user-rule  Copy the visible Cursor User Rule to the clipboard
  --help         Show this help`);
  process.exit(0);
}

const known = new Set(["--install", "--login", "--smoke-test", "--copy-user-rule"]);
const unknown = [...args].filter(arg => !known.has(arg));
if (unknown.length) {
  console.error(`Unknown option(s): ${unknown.join(", ")}`);
  process.exit(2);
}

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(scriptDir, "..");
const isWindows = process.platform === "win32";
const isWsl = !isWindows && (
  Boolean(process.env.WSL_DISTRO_NAME) ||
  (existsSync("/proc/version") && /microsoft/i.test(readFileSync("/proc/version", "utf8")))
);
const environment = isWindows ? "native Windows" : isWsl ? "WSL2" : "Linux/macOS";

const required = [
  ".cursor/hooks.json",
  ".cursor/hooks/enforce-codex-wrapper.js",
  ".cursor/rules/codex-orchestrator.mdc",
  ".cursor/codex-a2a-user-rule.txt",
  ".cursor/commands/delegate-to-codex.md",
  "scripts/invoke-codex-agent.ps1",
  "scripts/invoke-codex-agent.sh"
];

const run = (command, commandArgs, options = {}) => {
  const result = spawnSync(command, commandArgs, {
    cwd: repoRoot,
    stdio: "inherit",
    shell: false,
    ...options
  });
  if (result.error) {
    console.error(`${command} failed: ${result.error.message}`);
    return result.status ?? 1;
  }
  return result.status ?? 1;
};

// npm global shims are .cmd files on Windows; invoke those static commands
// through cmd.exe because Node cannot portably spawn batch files directly.
const runTool = (tool, toolArgs) => isWindows
  ? run(process.env.ComSpec || "cmd.exe", ["/d", "/s", "/c", [tool, ...toolArgs].join(" ")])
  : run(tool, toolArgs);

console.log(`Cursor → Codex A2A setup (${environment})`);
for (const relative of required) {
  if (!existsSync(resolve(repoRoot, relative))) {
    console.error(`Missing required project file: ${relative}`);
    process.exit(3);
  }
}
console.log("✓ Project integration files are present");

if (args.has("--install")) {
  if (runTool("npm", ["install", "-g", "@openai/codex"]) !== 0) process.exit(4);
}

if (runTool("codex", ["--version"]) !== 0) {
  console.error("Codex is unavailable in this environment. Re-run with --install.");
  process.exit(5);
}

if (args.has("--login")) {
  if (runTool("codex", ["login"]) !== 0) process.exit(6);
}

if (runTool("codex", ["login", "status"]) !== 0) {
  console.error("Codex is not authenticated here. Re-run with --login.");
  process.exit(7);
}
console.log("✓ Codex is installed and authenticated in this environment");

if (args.has("--copy-user-rule")) {
  const ruleText = readFileSync(resolve(repoRoot, ".cursor/codex-a2a-user-rule.txt"), "utf8");
  const clipboard = isWindows || isWsl ? "clip.exe" : "pbcopy";
  const copied = spawnSync(clipboard, [], { input: ruleText, encoding: "utf8" });
  if (copied.status !== 0 || copied.error) {
    console.error(`Could not copy User Rule with ${clipboard}. Open .cursor/codex-a2a-user-rule.txt and copy it manually.`);
    process.exit(9);
  }
  console.log("✓ Visible Cursor User Rule copied to the clipboard");
}

if (args.has("--smoke-test")) {
  const task = "A2A setup smoke test. Do not edit files or run commands. Reply exactly: CODEX_A2A_OK";
  const status = isWindows
    ? run("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", resolve(scriptDir, "invoke-codex-agent.ps1"), "-ReadOnly", "-Task", task])
    : run("bash", [resolve(scriptDir, "invoke-codex-agent.sh"), "--read-only", "--task", task]);
  if (status !== 0) process.exit(8);
}

console.log(`
Cursor configuration
1. Reload the Cursor window after trusting this project's hooks.
2. In the editor, press Ctrl+Shift+J and check Rules, or inspect the chat's
   Active Rules indicator for codex-orchestrator. Cursor's separate Agents
   Window may not display file-backed rules even when they are active.
3. Use Cursor Agent normally; the advisory preference is always applied while
   Cursor retains its normal Read, Search, Edit, Delete, and Run tools.
4. Use /delegate-to-codex when you want to force a complete handoff for one task.
5. For a rule that is reliably visible in Cursor 3, rerun setup with
   --copy-user-rule, then paste into Settings → Rules → User Rules and save.
Setup complete for ${environment}.`);
