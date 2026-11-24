import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { parseArgs, resolveCodex } from "../invoke-codex-agent.mjs";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const hook = join(repoRoot, ".cursor/hooks/enforce-codex-wrapper.js");
const launcherFiles = ["invoke-codex-agent.mjs", "prepare-codex-static-catalog.mjs"];
const launch = (root, args) => [process.execPath, [join(root, "scripts/invoke-codex-agent.mjs"), ...args]];
const wrapperCommand = "node scripts/invoke-codex-agent.mjs --task 'run codex exec on the tests'";

const runHookRaw = input => JSON.parse(spawnSync(process.execPath, [hook], { input, encoding: "utf8" }).stdout);
const runHook = command => runHookRaw(JSON.stringify({ command }));

test("hook allows the checked wrapper and blocks a direct codex launch", () => {
  assert.equal(runHook(wrapperCommand).permission, "allow");
  assert.equal(runHook("node .\\scripts\\invoke-codex-agent.mjs -Task 'x'").permission, "allow");
  assert.equal(runHook("node .\\scripts\\invoke-codex-agent.mjs -Task 'x'; codex exec").permission, "deny");
  assert.equal(runHook("node scripts/invoke-codex-agent.mjs --task x && codex exec -").permission, "deny");
  assert.equal(runHook("echo scripts/invoke-codex-agent.mjs | codex exec -").permission, "deny");
  assert.equal(runHook("node scripts/invoke-codex-agent.mjs --task 'a; codex exec' && git status").permission, "allow");
  assert.equal(runHook("bash ./scripts/invoke-codex-agent.sh --task x; codex exec -").permission, "deny");
  assert.equal(runHook("codex exec --sandbox workspace-write -").permission, "deny");
  assert.equal(runHook("npx codex exec -").permission, "deny");
  assert.equal(runHook("git commit -m 'mention codex here'").permission, "allow");
});

test("hook allows empty input and checks input that is not JSON as the raw command", () => {
  assert.deepEqual(runHookRaw(""), { continue: true, permission: "allow" });
  assert.deepEqual(runHookRaw("null"), { continue: true, permission: "allow" });
  assert.deepEqual(runHookRaw("git status"), { continue: true, permission: "allow" });
  assert.deepEqual(runHookRaw("codex exec -"), {
    continue: true,
    permission: "deny",
    user_message: "Direct Codex execution is blocked in this project.",
    agent_message: "Use node scripts/invoke-codex-agent.mjs --task '<task>'; do not bypass the A2A wrapper."
  });
  assert.equal(runHook("codex exec -").permission, "deny");
});

// A throwaway Git repository holding a copy of the launcher, with a stub
// `codex` on PATH that records its arguments and the prompt it was sent. The
// stub is a Node script where npm installs Codex's, so on Windows the
// launcher finds it behind codex.cmd as it does the real one.
function sandbox(stubExit = 0) {
  const dir = mkdtempSync(join(tmpdir(), "codex-a2a-"));
  const root = join(dir, "repo");
  mkdirSync(join(root, "scripts"), { recursive: true });
  for (const file of launcherFiles) copyFileSync(join(repoRoot, "scripts", file), join(root, "scripts", file));
  spawnSync("git", ["init", "-q", root]);
  const bin = join(dir, "bin");
  mkdirSync(bin);
  const record = join(dir, "record");
  const stub = join(bin, "node_modules", "@openai", "codex", "bin", "codex.js");
  mkdirSync(dirname(stub), { recursive: true });
  writeFileSync(stub, `const { readFileSync, writeFileSync } = require("node:fs");
const record = ${JSON.stringify(record)};
writeFileSync(record + ".args", process.argv.slice(2).map(arg => arg + "\\n").join(""));
writeFileSync(record + ".stdin", readFileSync(0));
writeFileSync(record + ".depth", process.env.CURSOR_CODEX_A2A_DEPTH || "");
process.exit(${stubExit});
`);
  if (process.platform === "win32") {
    writeFileSync(join(bin, "codex.cmd"), `@"${process.execPath}" "${stub}" %*\r\n`);
  } else {
    writeFileSync(join(bin, "codex"), `#!/bin/sh\nexec "${process.execPath}" "${stub}" "$@"\n`);
    chmodSync(join(bin, "codex"), 0o755);
  }
  // Windows spells it Path; one PATH entry keeps the stub first everywhere.
  const env = Object.fromEntries(Object.entries(process.env).filter(([name]) => name.toUpperCase() !== "PATH"));
  Object.assign(env, { PATH: `${bin}${delimiter}${process.env.PATH}`, CODEX_HOME: join(dir, "codex-home") });
  delete env.CURSOR_CODEX_A2A_DEPTH;
  delete env.CURSOR_CODEX_MODEL;
  const gitDir = spawnSync("git", ["-C", root, "rev-parse", "--absolute-git-dir"], { encoding: "utf8" }).stdout.trim();
  return {
    root,
    gitDir,
    record,
    codexHome: env.CODEX_HOME,
    run: (args, extraEnv = {}) => {
      const [command, commandArgs] = launch(root, args);
      return spawnSync(command, commandArgs, { env: { ...env, ...extraEnv }, encoding: "utf8" });
    },
    cleanup: () => rmSync(dir, { recursive: true, force: true })
  };
}

test("launcher runs codex exec with the pinned model, static catalog and prompt", t => {
  const box = sandbox();
  t.after(box.cleanup);
  const result = box.run(["--task", "Say hello", "--read-only"]);
  assert.equal(result.status, 0, result.stderr);
  const catalog = join(box.gitDir, "cursor-codex-a2a/model-catalog.json");
  assert.deepEqual(readFileSync(`${box.record}.args`, "utf8").trimEnd().split("\n"), [
    "exec", "--cd", box.root, "--sandbox", "read-only", "--color", "never",
    "--model", "gpt-5.6-sol", "--disable", "remote_models",
    "-c", `model_catalog_json="${catalog.replace(/\\/g, "/")}"`, "-"
  ]);
  assert.ok(existsSync(catalog));
  const prompt = readFileSync(`${box.record}.stdin`, "utf8");
  assert.match(prompt, /^You are the sole implementation agent for this task\.\n\nTASK\nSay hello\n/);
  assert.ok(prompt.includes(`- Work only in: ${box.root}\n`));
  assert.match(prompt, /STARTING GIT STATUS\n\?\? scripts\/\n$/);
  assert.equal(readFileSync(`${box.record}.depth`, "utf8"), "1");
  assert.ok(!existsSync(join(box.gitDir, "cursor-codex-a2a.lock")), "lock released");
});

test("launcher honours --model, CURSOR_CODEX_MODEL and workspace-write by default", t => {
  const box = sandbox();
  t.after(box.cleanup);
  assert.equal(box.run(["-Task", "x", "-Model", "custom-model"]).status, 0);
  let args = readFileSync(`${box.record}.args`, "utf8").split("\n");
  assert.equal(args[args.indexOf("--model") + 1], "custom-model");
  assert.equal(args[args.indexOf("--sandbox") + 1], "workspace-write");
  assert.equal(box.run(["--task", "x"], { CURSOR_CODEX_MODEL: "env-model" }).status, 0);
  args = readFileSync(`${box.record}.args`, "utf8").split("\n");
  assert.equal(args[args.indexOf("--model") + 1], "env-model");
});

test("launcher builds the static catalog from the Codex model cache", t => {
  const box = sandbox();
  t.after(box.cleanup);
  const catalog = () => JSON.parse(readFileSync(join(box.gitDir, "cursor-codex-a2a/model-catalog.json"), "utf8")).models;
  const writeCache = text => {
    mkdirSync(box.codexHome, { recursive: true });
    writeFileSync(join(box.codexHome, "models_cache.json"), text);
  };

  // No cache: one fallback model under the selected slug.
  assert.equal(box.run(["--task", "x", "--model", "custom-model"]).status, 0);
  let models = catalog();
  assert.equal(models.length, 1);
  assert.equal(models[0].slug, "custom-model");
  assert.equal(models[0].display_name, "GPT-5.6-Sol");
  assert.equal(models[0].supports_reasoning_summaries, true);
  assert.equal(models[0].context_window, 272000);

  // Cached models are filled in and repaired; a missing selected model is put first.
  writeCache(JSON.stringify({ models: [{
    slug: "cached", display_name: "Cached", supports_reasoning_summaries: null, default_reasoning_summary: null,
    supported_reasoning_levels: "bad", experimental_supported_tools: null, shell_type: "", visibility: "",
    apply_patch_tool_type: "unified", context_window: 1000
  }] }));
  assert.equal(box.run(["--task", "x"]).status, 0);
  models = catalog();
  assert.deepEqual(models.map(model => model.slug), ["gpt-5.6-sol", "cached"]);
  assert.equal(models[0].display_name, "GPT-5.6-Sol");
  assert.deepEqual(
    [models[1].display_name, models[1].supports_reasoning_summaries, models[1].default_reasoning_summary,
      models[1].supported_reasoning_levels.length, models[1].experimental_supported_tools, models[1].shell_type,
      models[1].visibility, models[1].apply_patch_tool_type, models[1].context_window,
      models[1].availability_nux, models[1].upgrade, models[1].default_verbosity],
    ["Cached", true, "none", 4, [], "shell_command", "list", "freeform", 1000, null, null, null]
  );

  // The selected model already cached is not added again.
  assert.equal(box.run(["--task", "x", "--model", "cached"]).status, 0);
  assert.deepEqual(catalog().map(model => model.slug), ["cached"]);

  // An empty model list or an unreadable cache falls back, warning about the latter.
  writeCache(JSON.stringify({ models: [] }));
  assert.equal(box.run(["--task", "x", "--model", "empty-cache"]).status, 0);
  assert.deepEqual(catalog().map(model => model.slug), ["empty-cache"]);
  writeCache("{ not json");
  const broken = box.run(["--task", "x", "--model", "broken-cache"]);
  assert.equal(broken.status, 0, broken.stderr);
  assert.match(broken.stderr, /Warning: could not read .*models_cache\.json/);
  assert.deepEqual(catalog().map(model => model.slug), ["broken-cache"]);
});

test("launcher refuses bad arguments, nesting and a held lock", t => {
  const box = sandbox();
  t.after(box.cleanup);
  assert.equal(box.run([]).status, 2);
  assert.equal(box.run(["--bogus"]).status, 2);
  assert.equal(box.run(["--task"]).status, 2);
  assert.equal(box.run(["--task", "x"], { CURSOR_CODEX_A2A_DEPTH: "1" }).status, 3);
  mkdirSync(join(box.gitDir, "cursor-codex-a2a.lock"));
  const locked = box.run(["--task", "x"]);
  assert.equal(locked.status, 5);
  assert.match(locked.stderr, /Another Codex delegation is active/);
  assert.ok(!existsSync(`${box.record}.args`));
});

test("launcher releases the lock when the catalog folder cannot be created", t => {
  const box = sandbox();
  t.after(box.cleanup);
  writeFileSync(join(box.gitDir, "cursor-codex-a2a"), "not a folder");
  const result = box.run(["--task", "x"]);
  assert.equal(result.status, 8, result.stderr);
  assert.ok(!existsSync(join(box.gitDir, "cursor-codex-a2a.lock")), "lock released");
  assert.ok(!existsSync(`${box.record}.args`));
});

test("launcher releases the lock when the catalog file cannot be written", t => {
  const box = sandbox();
  t.after(box.cleanup);
  mkdirSync(join(box.gitDir, "cursor-codex-a2a/model-catalog.json"), { recursive: true });
  const result = box.run(["--task", "x"]);
  assert.equal(result.status, 8, result.stderr);
  assert.match(result.stderr, /Failed to prepare static Codex model catalog at /);
  assert.ok(!existsSync(join(box.gitDir, "cursor-codex-a2a.lock")), "lock released");
  assert.ok(!existsSync(`${box.record}.args`));
});

test("launcher passes on the codex exit status", t => {
  const box = sandbox(7);
  t.after(box.cleanup);
  assert.equal(box.run(["--task", "x"]).status, 7);
  assert.ok(!existsSync(join(box.gitDir, "cursor-codex-a2a.lock")), "lock released");
});

test("launcher prints help and parses both option spellings", () => {
  const help = spawnSync(process.execPath, [join(repoRoot, "scripts/invoke-codex-agent.mjs"), "--help"], { encoding: "utf8" });
  assert.equal(help.status, 0);
  assert.match(help.stdout, /--task, -Task <text>/);
  assert.deepEqual(parseArgs(["-Task", "t", "-ReadOnly"], {}), { task: "t", readOnly: true, model: "gpt-5.6-sol" });
  assert.deepEqual(parseArgs(["--task", "t", "--model", "m"], { CURSOR_CODEX_MODEL: "e" }), { task: "t", readOnly: false, model: "m" });
});

test("launcher resolves the real Codex CLI behind the Windows npm shim", t => {
  const dir = mkdtempSync(join(tmpdir(), "codex-path-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  writeFileSync(join(dir, "codex.cmd"), "");
  const js = join(dir, "node_modules", "@openai", "codex", "bin", "codex.js");
  assert.deepEqual(resolveCodex("win32", { Path: dir }, path => path === js), { command: process.execPath, prefixArgs: [js] });
  assert.match(resolveCodex("win32", { Path: dir }, () => false).error, /reinstall Codex/);
  assert.match(resolveCodex("win32", { Path: join(dir, "missing") }).error, /codex is not installed/);
  assert.match(resolveCodex("linux", { PATH: join(dir, "missing") }).error, /codex is not installed/);
});
