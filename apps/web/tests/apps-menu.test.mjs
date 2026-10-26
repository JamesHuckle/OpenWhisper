import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import test from "node:test";

const apps = await readFile(new URL("../lib/apps.ts", import.meta.url), "utf8");
const nav = await readFile(new URL("../components/nav.tsx", import.meta.url), "utf8");
const home = await readFile(new URL("../app/page.tsx", import.meta.url), "utf8");
const menu = await readFile(new URL("../components/apps-menu.tsx", import.meta.url), "utf8");

test("the apps menu offers OpenWhisper, Zoon Meetings and OpenProof, each with a page", () => {
  const hrefs = [...apps.matchAll(/href: "([^"]+)"/g)].map((m) => m[1]);
  assert.deepEqual(hrefs, ["/", "/zoon-meetings", "/openproof"]);
  for (const name of ["OpenWhisper", "Zoon Meetings", "OpenProof"]) assert.match(apps, new RegExp(`name: "${name}"`));
  for (const page of ["zoon-meetings", "openproof"]) {
    assert.ok(existsSync(new URL(`../app/${page}/page.tsx`, import.meta.url)), `app/${page}/page.tsx`);
  }
});

test("every page's top bar carries the apps menu, and the homepage shows all three", () => {
  assert.match(nav, /<AppsMenu current=\{current\} \/>/);
  assert.match(home, /<Family \/>/);
});

test("the apps menu only takes over arrow keys while focus is in the menu or its button", () => {
  assert.match(menu, /if \(!panel\.current\?\.contains\(focused\) && focused !== button\.current\) return;/);
});
