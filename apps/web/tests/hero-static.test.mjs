import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import test from "node:test";

const hero = await readFile(new URL("../components/hero.tsx", import.meta.url), "utf8");

test("the homepage headline and download buttons never wait on GitHub's API", () => {
  assert.match(hero, /export function Hero\(\)/);
  assert.doesNotMatch(hero, /async|await|fetch\(|api\.github\.com|getLatestVersion/);
  assert.ok(!existsSync(new URL("../lib/github.ts", import.meta.url)), "lib/github.ts is gone");
});

test("the hero links to the latest release on GitHub instead of showing a fetched version", () => {
  assert.match(hero, /href=\{RELEASES_URL\}/);
  assert.match(hero, /Latest release on GitHub/);
});
