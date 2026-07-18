import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const config = await readFile(new URL("../lib/config.ts", import.meta.url), "utf8");
const button = await readFile(
  new URL("../components/download-button.tsx", import.meta.url),
  "utf8",
);
const hero = await readFile(new URL("../components/hero.tsx", import.meta.url), "utf8");
const cta = await readFile(
  new URL("../components/cta-section.tsx", import.meta.url),
  "utf8",
);

test("publishes the stable signed Android APK URL", () => {
  assert.match(
    config,
    /ANDROID_DOWNLOAD_URL[\s\S]*releases\/latest\/download\/OpenWhisper-Android\.apk/,
  );
});

test("offers Android downloads in the reusable button and primary CTAs", () => {
  assert.match(button, /Download for Android/);
  assert.match(button, /ANDROID_DOWNLOAD_URL/);
  assert.match(hero, /platform="android"/);
  assert.match(cta, /platform="android"/);
});
