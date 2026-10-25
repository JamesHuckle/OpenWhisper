import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";

const dir = new URL("../public/zoon-meetings/", import.meta.url);
const feed = JSON.parse(await readFile(new URL("android-update.json", dir), "utf8"));
const page = await readFile(new URL("../app/zoon-meetings/page.tsx", import.meta.url), "utf8");

test("the Zoon Meetings update feed points at the published APK and matches it", async () => {
  assert.equal(feed.apk, "https://open-whisper.vercel.app/zoon-meetings/ZoonMeetings-Android.apk");
  assert.ok(Number.isInteger(feed.versionCode) && feed.versionCode > 0);
  const apk = await readFile(new URL("ZoonMeetings-Android.apk", dir));
  assert.equal(createHash("sha256").update(apk).digest("hex"), feed.sha256);
});

test("the Zoon Meetings page offers the APK download", () => {
  assert.match(page, /\/zoon-meetings\/ZoonMeetings-Android\.apk/);
  assert.match(page, /Download for Android/);
});
