#!/usr/bin/env node
"use strict";

// Checked Cursor → Codex launcher for native Windows, WSL and Linux/macOS:
// one delegation at a time (lock in the Git dir), no nesting, a pinned model
// and a static model catalog, then `codex exec` with the task on stdin.

import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, unlinkSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
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

// Desktop/CLI version skew often writes models_cache.json without fields the
// CLI deserializer requires (e.g. supports_reasoning_summaries), which forces a
// cold refresh child every ~TTL that times out, leaks pipe fds and kills long
// delegations (exit 4294967295 on Windows). A static catalog built from the
// cache, with those fields filled in, skips that refresh loop.
const FALLBACK_MODEL = {
  slug: "gpt-5.6-sol",
  display_name: "GPT-5.6-Sol",
  description: "Latest frontier agentic coding model.",
  default_reasoning_level: "high",
  supported_reasoning_levels: [
    { effort: "low", description: "Fast responses with lighter reasoning" },
    { effort: "medium", description: "Balances speed and reasoning depth" },
    { effort: "high", description: "Greater reasoning depth for complex problems" },
    { effort: "xhigh", description: "Extra high reasoning depth" }
  ],
  shell_type: "shell_command",
  visibility: "list",
  supported_in_api: true,
  priority: 1,
  availability_nux: null,
  upgrade: null,
  base_instructions: "",
  supports_reasoning_summaries: true,
  default_reasoning_summary: "none",
  support_verbosity: false,
  default_verbosity: null,
  apply_patch_tool_type: "freeform",
  truncation_policy: { mode: "tokens", limit: 10000 },
  supports_parallel_tool_calls: true,
  context_window: 272000,
  max_context_window: 272000,
  effective_context_window_percent: 95,
  experimental_supported_tools: [],
  input_modalities: ["text", "image"]
};

function normalizeModel(raw) {
  const model = { ...FALLBACK_MODEL, ...raw };
  model.supports_reasoning_summaries ??= true;
  model.default_reasoning_summary ??= "none";
  if (!Array.isArray(model.supported_reasoning_levels)) {
    model.supported_reasoning_levels = FALLBACK_MODEL.supported_reasoning_levels;
  }
  if (!Array.isArray(model.experimental_supported_tools)) model.experimental_supported_tools = [];
  if (!model.shell_type) model.shell_type = "shell_command";
  if (!model.visibility) model.visibility = "list";
  if (model.apply_patch_tool_type === "unified") model.apply_patch_tool_type = "freeform";
  return model;
}

// The cached models with the selected one first if the cache lacks it; just
// the selected one when there is no usable cache.
function loadModels(slug) {
  const fallback = () => [normalizeModel({ slug })];
  const cachePath = resolve(process.env.CODEX_HOME || resolve(homedir(), ".codex"), "models_cache.json");
  if (!existsSync(cachePath)) return fallback();
  try {
    const parsed = JSON.parse(readFileSync(cachePath, "utf8"));
    const models = Array.isArray(parsed?.models) ? parsed.models.map(normalizeModel) : [];
    if (!models.length) return fallback();
    if (!models.some(model => model.slug === slug)) models.unshift(...fallback());
    return models;
  } catch (error) {
    console.error(`Warning: could not read ${cachePath}: ${error.message}`);
    return fallback();
  }
}

function writeStaticCatalog(path, slug) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify({ models: loadModels(slug) }, null, 2)}\n`, "utf8");
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
  try {
    writeStaticCatalog(catalogPath, options.model);
  } catch (error) {
    console.error(`Failed to prepare static Codex model catalog at ${catalogPath}: ${error.message}`);
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
