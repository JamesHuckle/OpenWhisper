import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const hook = join(repoRoot, ".cursor/hooks/enforce-codex-wrapper.js");
const launcherFiles = ["invoke-codex-agent.sh", "prepare-codex-static-catalog.mjs"];
const launch = (root, args) => ["bash", [join(root, "scripts/invoke-codex-agent.sh"), ...args]];
const wrapperCommand = "bash ./scripts/invoke-codex-agent.sh --task 'run codex exec on the tests'";

const runHook = command => JSON.parse(
  spawnSync(process.execPath, [hook], { input: JSON.stringify({ command }), encoding: "utf8" }).stdout
);

test("hook allows the checked wrapper and blocks a direct codex launch", () => {
  assert.equal(runHook(wrapperCommand).permission, "allow");
  assert.equal(runHook("codex exec --sandbox workspace-write -").permission, "deny");
  assert.equal(runHook("npx codex exec -").permission, "deny");
  assert.equal(runHook("git commit -m 'mention codex here'").permission, "allow");
});

// A throwaway Git repository holding a copy of the launcher, with a stub
// `codex` on PATH that records its arguments and the prompt it was sent.
function sandbox(stubExit = 0) {
  const dir = mkdtempSync(join(tmpdir(), "codex-a2a-"));
  const root = join(dir, "repo");
  mkdirSync(join(root, "scripts"), { recursive: true });
  for (const file of launcherFiles) copyFileSync(join(repoRoot, "scripts", file), join(root, "scripts", file));
  spawnSync("git", ["init", "-q", root]);
  const bin = join(dir, "bin");
  mkdirSync(bin);
  const record = join(dir, "record");
  writeFileSync(join(bin, "codex"), `#!/bin/sh
printf '%s\\n' "$@" > "${record}.args"
cat > "${record}.stdin"
printf '%s' "$CURSOR_CODEX_A2A_DEPTH" > "${record}.depth"
exit ${stubExit}
`);
  chmodSync(join(bin, "codex"), 0o755);
  const env = { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}`, CODEX_HOME: join(dir, "codex-home") };
  delete env.CURSOR_CODEX_A2A_DEPTH;
  delete env.CURSOR_CODEX_MODEL;
  const gitDir = spawnSync("git", ["-C", root, "rev-parse", "--absolute-git-dir"], { encoding: "utf8" }).stdout.trim();
  return {
    root,
    gitDir,
    record,
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
    "-c", `model_catalog_json="${catalog}"`, "-"
  ]);
  assert.ok(existsSync(catalog));
  const prompt = readFileSync(`${box.record}.stdin`, "utf8");
  assert.match(prompt, /^You are the sole implementation agent for this task\.\n\nTASK\nSay hello\n/);
  assert.match(prompt, new RegExp(`- Work only in: ${box.root}\n`));
  assert.match(prompt, /STARTING GIT STATUS\n\?\? scripts\/\n$/);
  assert.equal(readFileSync(`${box.record}.depth`, "utf8"), "1");
});

test("launcher honours --model, CURSOR_CODEX_MODEL and workspace-write by default", t => {
  const box = sandbox();
  t.after(box.cleanup);
  assert.equal(box.run(["-Task", "x", "-Model", "custom-model"]).status, 0);
  let args = readFileSync(`${box.record}.args`, "utf8").split("\n");
  assert.equal(args[args.indexOf("--model") + 1], "custom-model");
  assert.equal(args[args.indexOf("--sandbox") + 1], "workspace-write");
  rmSync(join(box.gitDir, "cursor-codex-a2a.lock"), { recursive: true, force: true });
  assert.equal(box.run(["--task", "x"], { CURSOR_CODEX_MODEL: "env-model" }).status, 0);
  args = readFileSync(`${box.record}.args`, "utf8").split("\n");
  assert.equal(args[args.indexOf("--model") + 1], "env-model");
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

test("launcher passes on the codex exit status", t => {
  const box = sandbox(7);
  t.after(box.cleanup);
  assert.equal(box.run(["--task", "x"]).status, 7);
});
