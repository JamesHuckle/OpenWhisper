#!/usr/bin/env node
"use strict";

// Checked Cursor → Codex launcher for native Windows, WSL and Linux/macOS:
// one delegation at a time (lock in the Git dir), no nesting, a pinned model
// and a static model catalog, then `codex exec` with the task on stdin.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, unlinkSync, writeFileSync } from "node:fs";
import { delimiter, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const usage = `Usage: node scripts/invoke-codex-agent.mjs --task <text> [options]

Options:
  --task, -Task <text>     Complete task and acceptance criteria (required)
  --read-only, -ReadOnly   Run Codex in a read-only sandbox (default: workspace-write)
  --model, -Model <slug>   Model slug (default: CURSOR_CODEX_MODEL or gpt-5.6-sol)
  --help                   Show this help`;

export function parseArgs(argv, env = process.env) {
  const options = { task: "", readOnly: false, model: env.CURSOR_CODEX_MODEL || "gpt-5.6-sol" };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") return { help: true };
    if (arg === "--read-only" || arg === "-ReadOnly") options.readOnly = true;
    else if (arg === "--task" || arg === "-Task" || arg === "--model" || arg === "-Model") {
      if (i + 1 >= argv.length) return { error: `${arg} requires a value` };
      options[/task/i.test(arg) ? "task" : "model"] = argv[++i];
    } else if (arg === "--") break;
    else return { error: `Unknown argument: ${arg}` };
  }
  return options.task ? options : { error: "--task is required" };
}

function findOnPath(name, env) {
  for (const dir of (env.PATH || env.Path || "").split(delimiter)) {
    const candidate = dir && join(dir, name);
    if (candidate && existsSync(candidate)) return candidate;
  }
  return null;
}

// Returns { command, prefixArgs } for the real Codex CLI, or { error }. On
// Windows npm installs codex.cmd, which Node cannot spawn without a shell, so
// run its codex.js with this Node instead (or a native codex.exe).
export function resolveCodex(platform = process.platform, env = process.env, exists = existsSync) {
  if (platform !== "win32") {
    return findOnPath("codex", env) ? { command: "codex", prefixArgs: [] } : { error: "codex is not installed or is not on PATH." };
  }
  const shim = findOnPath("codex.cmd", env);
  if (shim) {
    const js = join(dirname(shim), "node_modules", "@openai", "codex", "bin", "codex.js");
    if (exists(js)) return { command: process.execPath, prefixArgs: [js] };
    return { error: `Found ${shim} but not ${js}; reinstall Codex with npm install -g @openai/codex.` };
  }
  const exe = findOnPath("codex.exe", env);
  return exe ? { command: exe, prefixArgs: [] } : { error: "codex is not installed (expected codex.cmd or codex.exe on PATH)." };
}

function fail(message, code) {
  console.error(message);
  process.exit(code);
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  if (options.help) {
    console.log(usage);
    return;
  }
  if (options.error) fail(`${options.error}\n\n${usage}`, 2);
  if (process.env.CURSOR_CODEX_A2A_DEPTH) {
    fail("Nested agent delegation is forbidden (CURSOR_CODEX_A2A_DEPTH is already set).", 3);
  }

  const scriptDir = dirname(fileURLToPath(import.meta.url));
  const repoRoot = resolve(scriptDir, "..");
  const git = spawnSync("git", ["-C", repoRoot, "rev-parse", "--absolute-git-dir"], { encoding: "utf8" });
  const gitDir = git.status === 0 ? git.stdout.trim() : "";
  if (!gitDir) fail("The A2A wrapper must run inside a Git worktree.", 4);

  const lockPath = join(gitDir, "cursor-codex-a2a.lock");
  try {
    writeFileSync(lockPath, `pid=${process.pid}\nstarted=${new Date().toISOString()}\nplatform=${process.platform}\n`, { flag: "wx" });
  } catch {
    fail(`Another Codex delegation is active, or a stale lock exists at ${lockPath}.\n` +
      "Verify no Codex task is running before removing the stale lock.", 5);
  }
  const releaseLock = () => { try { unlinkSync(lockPath); } catch {} };
  const exit = code => { releaseLock(); process.exit(code); };

  const codex = resolveCodex();
  if (codex.error) {
    console.error(codex.error);
    exit(6);
  }

  const catalogPath = join(gitDir, "cursor-codex-a2a", "model-catalog.json");
  mkdirSync(dirname(catalogPath), { recursive: true });
  const prepared = spawnSync(process.execPath, [join(scriptDir, "prepare-codex-static-catalog.mjs"), catalogPath, options.model], {
    stdio: ["ignore", "ignore", "inherit"]
  });
  if (prepared.status !== 0 || !existsSync(catalogPath)) {
    console.error(`Failed to prepare static Codex model catalog at ${catalogPath}.`);
    exit(8);
  }

  const status = spawnSync("git", ["-C", repoRoot, "status", "--short"], { encoding: "utf8" }).stdout?.trimEnd() || "(clean)";
  const prompt = `You are the sole implementation agent for this task.

TASK
${options.task}

CONTRACT
- Work only in: ${repoRoot}
- Preserve unrelated user changes; the starting git status is included below.
- Do not invoke Cursor, Codex, invoke-codex-agent.mjs, or another agent.
- Do not commit, push, open pull requests, or modify Git configuration.
- Inspect before editing, implement the task completely, and run proportionate verification.
- End with a concise list of changed files, verification performed, and remaining risks.

STARTING GIT STATUS
${status}
`;

  // Static catalog => StaticModelsManager; skip refresh-child FD leaks that
  // kill long-running exec when models_cache TTL/schema skew forces a cold
  // refresh. TOML wants forward slashes in the Windows catalog path.
  const child = spawn(codex.command, [
    ...codex.prefixArgs,
    "exec",
    "--cd", repoRoot,
    "--sandbox", options.readOnly ? "read-only" : "workspace-write",
    "--color", "never",
    "--model", options.model,
    "--disable", "remote_models",
    "-c", `model_catalog_json="${catalogPath.replace(/\\/g, "/")}"`,
    "-"
  ], { stdio: ["pipe", "inherit", "inherit"], env: { ...process.env, CURSOR_CODEX_A2A_DEPTH: "1" } });

  for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => child.kill(signal));
  child.on("error", error => {
    console.error(`Failed to start Codex: ${error.message}`);
    exit(6);
  });
  child.on("exit", (code, signal) => {
    if (signal) console.error(`Codex was stopped by ${signal}.`);
    exit(code ?? 1);
  });
  child.stdin.on("error", () => {});
  child.stdin.end(prompt);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
