import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";

import {
  androidFeed,
  androidVersionCode,
  applyVersion,
  checkFeeds,
  checkPublish,
  checkReleaseAssets,
  committedVersion,
  gitIn,
  isAppChange,
  latestTag,
  needsRelease,
  nextVersion,
  planRelease,
  releaseAssetNames,
  repositoryRoot,
  setVersion,
  stageRelease,
  versionFiles,
  windowsFeed,
} from "../release.mjs";

describe("release version", () => {
  it("takes the highest v-tag, not the newest by name", () => {
    assert.equal(latestTag(["v0.2.9", "v0.2.27", "v0.2.10", "nightly", "0.9.0"]), "v0.2.27");
    assert.equal(latestTag([]), null);
  });

  it("is the last tag + 1, unless the committed version was raised by hand", () => {
    assert.equal(nextVersion("v0.2.27", "0.2.27"), "0.2.28");
    assert.equal(nextVersion("v0.2.27", "0.2.20"), "0.2.28");
    assert.equal(nextVersion("v0.2.27", "0.3.0"), "0.3.0");
    assert.equal(nextVersion(null, "0.2.27"), "0.2.27");
  });

  it("gives Android a versionCode that grows with every release", () => {
    assert.equal(androidVersionCode("0.2.28"), 2028);
    // 0.2.27 shipped with versionCode 29; the next release must be higher.
    assert.ok(androidVersionCode("0.2.28") > 29);
    assert.ok(androidVersionCode("0.3.0") > androidVersionCode("0.2.999"));
    assert.ok(androidVersionCode("1.0.0") > androidVersionCode("0.999.999"));
    assert.throws(() => androidVersionCode("0.2.1000"));
  });
});

describe("release decision", () => {
  it("releases for app code and not for the website, docs or tests", () => {
    for (const path of [
      "apps/android/app/src/main/java/com/openwhisper/android/MainActivity.kt",
      "apps/desktop/src/main.ts",
      "apps/desktop/src-tauri/src/lib.rs",
      "apps/desktop/src-tauri/src/transcriber.rs",
      "apps/desktop/src-tauri/core/lib.rs",
      "packages/protocol/audio-rules.json",
      "scripts/build-windows-installer.ps1",
    ]) {
      assert.equal(isAppChange(path), true, path);
    }
    for (const path of [
      "apps/web/app/page.tsx",
      "README.md",
      "docs/android.md",
      "apps/desktop/src/ui-regressions.test.ts",
      "apps/android/app/src/test/java/com/openwhisper/android/AudioRulesTest.kt",
      "packages/protocol/README.md",
      ".github/workflows/release.yml",
    ]) {
      assert.equal(isAppChange(path), false, path);
    }
  });

  it("skips website-only pushes and always releases when there is no tag yet", () => {
    assert.equal(needsRelease("v0.2.27", ["apps/web/app/page.tsx", "README.md"]), false);
    assert.equal(needsRelease("v0.2.27", ["apps/web/app/page.tsx", "apps/android/app/build.gradle.kts"]), true);
    assert.equal(needsRelease(null, []), true);
  });
});

// A throwaway repository: v0.2.27 released at `released`, then main moves on
// to `next` (app code); `old` is a branch cut before the release.
function releaseHistory() {
  const root = mkdtempSync(join(tmpdir(), "openwhisper-history-"));
  const plain = gitIn(root);
  const run = (...args) => plain(
    "-c", "user.name=Test", "-c", "user.email=test@example.invalid",
    "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", "-c", "core.hooksPath=/dev/null",
    ...args,
  );
  const commit = (path, message) => {
    mkdirSync(dirname(join(root, path)), { recursive: true });
    writeFileSync(join(root, path), `${message}\n`);
    run("add", "-A");
    run("commit", "-q", "-m", message);
    return run("rev-parse", "HEAD");
  };
  run("init", "-q", "-b", "main");
  const base = commit("apps/desktop/src/main.ts", "base");
  const released = commit("apps/desktop/src/main.ts", "released");
  run("tag", "v0.2.27", released);
  const website = commit("apps/web/app/page.tsx", "website");
  const next = commit("apps/android/app/build.gradle.kts", "next");
  run("checkout", "-q", "-b", "old", base);
  const old = commit("apps/desktop/src/lib.ts", "old branch");
  run("checkout", "-q", "main");
  run("update-ref", "refs/remotes/origin/main", next);
  return { root, run, base, released, website, next, old, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

describe("release order", () => {
  const published = (tag, extra = {}) => ({ tag_name: tag, draft: false, assets: [], ...extra });

  it("releases a commit that comes after the last release with new app code", () => {
    const history = releaseHistory();
    try {
      const plan = planRelease({ run: history.run, committed: "0.2.27", head: history.next });
      assert.equal(plan.release, true);
      assert.equal(plan.version, "0.2.28");
      const website = planRelease({ run: history.run, committed: "0.2.27", head: history.website });
      assert.equal(website.release, false);
      assert.match(website.message, /No app code changed/);
      assert.deepEqual(
        checkPublish({ run: history.run, version: "0.2.28", target: history.next, releases: [published("v0.2.27")] }),
        { action: "publish" },
      );
    } finally {
      history.cleanup();
    }
  });

  it("never releases an old run re-run after a newer release", () => {
    const history = releaseHistory();
    try {
      history.run("tag", "v0.2.28", history.next);
      for (const head of [history.released, history.website, history.next]) {
        const plan = planRelease({ run: history.run, committed: "0.2.27", head });
        assert.equal(plan.release, false, head);
        assert.match(plan.message, /already in v0\.2\.28/);
      }
      // Even if a stale plan reaches publish, it refuses to mark older code latest.
      const releases = [published("v0.2.27"), published("v0.2.28")];
      assert.throws(
        () => checkPublish({ run: history.run, version: "0.2.29", target: history.website, releases }),
        /already in v0\.2\.28/,
      );
      assert.throws(
        () => checkPublish({ run: history.run, version: "0.2.28", target: history.website, releases: [published("v0.2.27"), published("v0.2.29")] }),
        /not newer than v0\.2\.29/,
      );
    } finally {
      history.cleanup();
    }
  });

  it("fails on main, and only tests on other branches, when history split from the last release", () => {
    const history = releaseHistory();
    try {
      assert.throws(
        () => planRelease({ run: history.run, committed: "0.2.27", head: history.old }),
        /does not contain v0\.2\.27/,
      );
      assert.equal(planRelease({ run: history.run, committed: "0.2.27", head: history.old, onMain: false }).release, false);
      // A commit that never reached main is never published.
      assert.throws(
        () => checkPublish({ run: history.run, version: "0.2.28", target: history.old, releases: [published("v0.2.27")] }),
        /not on main/,
      );
      history.run("update-ref", "refs/remotes/origin/main", history.old);
      assert.throws(
        () => checkPublish({ run: history.run, version: "0.2.28", target: history.old, releases: [published("v0.2.27")] }),
        /does not contain v0\.2\.27/,
      );
    } finally {
      history.cleanup();
    }
  });

  it("finishes a re-run after publishing without marking the release latest again", () => {
    const history = releaseHistory();
    try {
      history.run("tag", "v0.2.28", history.next);
      const release = published("v0.2.28", {
        assets: [
          { name: "a.exe", state: "uploaded", size: 10 },
          { name: "b.apk", state: "uploaded", size: 20 },
        ],
      });
      const result = checkPublish({
        run: history.run, version: "0.2.28", target: history.next, releases: [published("v0.2.27"), release],
      });
      assert.equal(result.action, "finish");
      assert.equal(result.newest, true);
      assert.deepEqual(checkReleaseAssets(result.release, { "a.exe": 10, "b.apk": 20 }), []);
      assert.deepEqual(checkReleaseAssets(result.release, { "a.exe": 10, "b.apk": 21, "c.json": 5 }), [
        "v0.2.28 has a b.apk of 20 bytes, not 21.",
        "v0.2.28 is missing c.json.",
      ]);
      // A file GitHub has not finished receiving counts as missing.
      assert.deepEqual(
        checkReleaseAssets({ tag_name: "v0.2.28", assets: [{ name: "a.exe", state: "starter", size: 10 }] }, { "a.exe": 10 }),
        ["v0.2.28 is missing a.exe."],
      );
      assert.equal(
        checkPublish({
          run: history.run, version: "0.2.28", target: history.next, releases: [release, published("v0.2.29")],
        }).newest,
        false,
      );
      // The same tag from another commit is never touched.
      assert.throws(
        () => checkPublish({ run: history.run, version: "0.2.28", target: history.website, releases: [release] }),
        /already published from commit/,
      );
      // An unpublished tag on another commit is never released from this one.
      history.run("tag", "v0.2.30", history.website);
      assert.throws(
        () => checkPublish({ run: history.run, version: "0.2.30", target: history.next, releases: [published("v0.2.27")] }),
        /Tag v0\.2\.30 already points to commit/,
      );
      // A leftover draft does not count as published.
      assert.deepEqual(
        checkPublish({
          run: history.run, version: "0.2.28", target: history.next,
          releases: [published("v0.2.27"), { tag_name: "v0.2.28", draft: true }],
        }),
        { action: "publish" },
      );
    } finally {
      history.cleanup();
    }
  });
});

describe("version files", () => {
  it("sets one version in every desktop and Android file without other changes", () => {
    const root = mkdtempSync(join(tmpdir(), "openwhisper-version-"));
    try {
      for (const path of Object.values(versionFiles)) {
        mkdirSync(dirname(join(root, path)), { recursive: true });
        cpSync(join(repositoryRoot, path), join(root, path));
      }
      const before = committedVersion(root);
      setVersion(root, "7.8.9");

      const read = (name) => readFileSync(join(root, versionFiles[name]), "utf8");
      assert.equal(JSON.parse(read("tauriConfig")).version, "7.8.9");
      assert.equal(JSON.parse(read("packageJson")).version, "7.8.9");
      const lock = JSON.parse(read("packageLock"));
      assert.equal(lock.version, "7.8.9");
      assert.equal(lock.packages[""].version, "7.8.9");
      assert.match(read("cargoToml"), /^version = "7\.8\.9"$/m);
      assert.match(read("cargoLock"), /name = "openwhisper_desktop"\nversion = "7\.8\.9"/);
      assert.match(read("androidGradle"), /versionCode = 7008009\n/);
      assert.match(read("androidGradle"), /versionName = "7\.8\.9"\n/);

      // Only the app's own version lines change, never a dependency's.
      for (const [name, path] of Object.entries(versionFiles)) {
        const original = readFileSync(join(repositoryRoot, path), "utf8").split("\n");
        const updated = read(name).split("\n");
        assert.equal(updated.length, original.length, path);
        const changed = updated.filter((line, index) => line !== original[index]);
        assert.ok(changed.length >= 1 && changed.length <= 2, `${path}: ${changed.join(" | ")}`);
        assert.ok(changed.every((line) => /7\.8\.9|7008009/.test(line)), path);
        assert.ok(original.some((line) => line.includes(before)), path);
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("fails rather than building with a version it could not write", () => {
    assert.throws(() => applyVersion("androidGradle", "versionName = \"1.0.0\"", "1.0.1"), /versionCode|version/);
    assert.throws(() => applyVersion("cargoLock", "[[package]]\nname = \"other\"\nversion = \"1\"", "1.0.1"));
  });
});

describe("update feeds", () => {
  const repository = "owner/OpenWhisper";
  const version = "0.2.28";
  const signature = "c2lnbmF0dXJl\n";
  const apkSha256 = "ab".repeat(32);

  it("points both feeds at this release's files", () => {
    const windows = windowsFeed({ repository, version, signature, pubDate: "2026-11-15T10:00:00Z" });
    const android = androidFeed({ repository, version, sha256: apkSha256 });
    assert.equal(
      windows.platforms["windows-x86_64"].url,
      "https://github.com/owner/OpenWhisper/releases/download/v0.2.28/OpenWhisper_0.2.28_x64-setup.exe",
    );
    assert.equal(windows.platforms["windows-x86_64"].signature, "c2lnbmF0dXJl");
    assert.equal(android.apkUrl, "https://github.com/owner/OpenWhisper/releases/download/v0.2.28/OpenWhisper-Android.apk");
    assert.equal(android.versionCode, 2028);
    assert.deepEqual(checkFeeds({ repository, version, signature, apkSha256, windows, android }), []);
  });

  it("publishes eight files under fixed public names", () => {
    // The website's latest-download links use the two names without a version.
    assert.deepEqual(Object.values(releaseAssetNames(version)).sort(), [
      "OpenWhisper-Android-update.json",
      "OpenWhisper-Android-v0.2.28.apk",
      "OpenWhisper-Android-v0.2.28.apk.sha256",
      "OpenWhisper-Android.apk",
      "OpenWhisper_0.2.28_x64-setup.exe",
      "OpenWhisper_0.2.28_x64-setup.exe.sig",
      "OpenWhisper_x64-setup.exe",
      "latest.json",
    ]);
    const website = readRepo("apps/web/lib/config.ts");
    assert.ok(website.includes("releases/latest/download/OpenWhisper_x64-setup.exe"));
    assert.ok(website.includes("releases/latest/download/OpenWhisper-Android.apk"));
  });

  it("stages all eight files from the four versioned builds", () => {
    const directory = mkdtempSync(join(tmpdir(), "openwhisper-stage-"));
    try {
      const names = releaseAssetNames(version);
      writeFileSync(join(directory, names.windowsInstaller), "installer bytes");
      writeFileSync(join(directory, names.windowsSignature), signature);
      writeFileSync(join(directory, names.androidApk), "apk bytes");
      const builtApkSha256 = createHash("sha256").update("apk bytes").digest("hex");
      writeFileSync(join(directory, names.androidChecksum), `${builtApkSha256}  ${names.androidApk}\n`);
      const staged = stageRelease({ directory, repository, version, pubDate: "2026-11-15T10:00:00Z" });

      assert.deepEqual(readdirSync(directory).sort(), Object.values(names).sort());
      assert.deepEqual(staged.uploads.sort(), Object.values(names).sort());
      const read = (name) => readFileSync(join(directory, name), "utf8");
      assert.equal(read(names.windowsStableInstaller), "installer bytes");
      assert.equal(read(names.androidStableApk), "apk bytes");
      assert.equal(staged.signature, "c2lnbmF0dXJl");
      assert.equal(staged.apkSha256, createHash("sha256").update("apk bytes").digest("hex"));

      const windows = JSON.parse(read(names.windowsFeed));
      const android = JSON.parse(read(names.androidFeed));
      assert.equal(
        windows.platforms["windows-x86_64"].url,
        "https://github.com/owner/OpenWhisper/releases/download/v0.2.28/OpenWhisper_0.2.28_x64-setup.exe",
      );
      assert.equal(android.apkUrl, "https://github.com/owner/OpenWhisper/releases/download/v0.2.28/OpenWhisper-Android.apk");
      assert.deepEqual(checkFeeds({ repository, version, signature, apkSha256: staged.apkSha256, windows, android }), []);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  it("refuses to stage without a versioned build, with an empty signature or an APK unlike its checksum", () => {
    const directory = mkdtempSync(join(tmpdir(), "openwhisper-stage-"));
    try {
      const names = releaseAssetNames(version);
      const stage = () => stageRelease({ directory, repository, version, pubDate: "2026-11-15T10:00:00Z" });
      writeFileSync(join(directory, names.windowsInstaller), "installer bytes");
      writeFileSync(join(directory, names.windowsSignature), " \n");
      writeFileSync(join(directory, names.androidApk), "apk bytes");
      assert.throws(stage, /Missing release file: OpenWhisper-Android-v0\.2\.28\.apk\.sha256/);
      writeFileSync(join(directory, names.androidChecksum), "checksum line\n");
      assert.throws(stage, /signature is empty/);
      writeFileSync(join(directory, names.windowsSignature), signature);
      // An APK that changed after the build wrote its checksum.
      const builtSha256 = createHash("sha256").update("signed apk bytes").digest("hex");
      writeFileSync(join(directory, names.androidChecksum), `${builtSha256}  ${names.androidApk}\n`);
      assert.throws(stage, /does not match OpenWhisper-Android-v0\.2\.28\.apk\.sha256/);
      assert.equal(existsSync(join(directory, names.androidFeed)), false);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  it("refuses feeds from another version or with the wrong checksum", () => {
    const windows = windowsFeed({ repository, version: "0.2.27", signature, pubDate: "2026-11-15T10:00:00Z" });
    const android = { ...androidFeed({ repository, version, sha256: apkSha256 }), sha256: "00".repeat(32) };
    const problems = checkFeeds({ repository, version, signature, apkSha256, windows, android });
    assert.equal(problems.length, 2);
    assert.match(problems[0], /^latest\.json is wrong\.[\s\S]*actual {3}\{"version":"0\.2\.27"/);
    assert.match(problems[1], /^OpenWhisper-Android-update\.json is wrong\.[\s\S]*actual .*"sha256":"(00){32}"/);
  });

  it("refuses a feed with any of its version, link, signature or checksum changed", () => {
    const pubDate = "2026-11-15T10:00:00Z";
    const feeds = () => ({
      windows: windowsFeed({ repository, version, signature, pubDate }),
      android: androidFeed({ repository, version, sha256: apkSha256 }),
    });
    const changes = [
      (feed) => { feed.windows.version = "0.2.27"; },
      (feed) => { feed.windows.platforms["windows-x86_64"].url += "?old"; },
      (feed) => { feed.windows.platforms["windows-x86_64"].signature = "b3RoZXI="; },
      (feed) => { feed.android.versionName = "0.2.27"; },
      (feed) => { feed.android.versionCode = 2027; },
      (feed) => { feed.android.apkUrl += "?old"; },
      (feed) => { feed.android.sha256 = "00".repeat(32); },
    ];
    for (const change of changes) {
      const feed = feeds();
      change(feed);
      assert.notDeepEqual(checkFeeds({ repository, version, signature, apkSha256, ...feed }), [], change.toString());
    }
  });

  it("refuses changed release notes, added fields or platforms and a bad publish date", () => {
    const feeds = (pubDate = "2026-11-15T10:00:00Z") => ({
      windows: windowsFeed({ repository, version, signature, pubDate }),
      android: androidFeed({ repository, version, sha256: apkSha256 }),
    });
    const check = (feed) => checkFeeds({ repository, version, signature, apkSha256, ...feed });
    assert.deepEqual(check(feeds("2027-01-31T23:59:59Z")), []);
    const changes = [
      (feed) => { feed.windows.notes = "Other notes"; },
      (feed) => { feed.windows.platforms["darwin-aarch64"] = feed.windows.platforms["windows-x86_64"]; },
      (feed) => { feed.android.releaseNotes = "Other notes"; },
      (feed) => { feed.android.minSdk = 28; },
      (feed) => { delete feed.windows.pub_date; },
    ];
    for (const change of changes) {
      const feed = feeds();
      change(feed);
      assert.equal(check(feed).length >= 1, true, change.toString());
    }
    for (const pubDate of ["not-a-date", "2026-02-30T10:00:00Z", "2026-11-15", "2026-11-15T10:00:00.123Z", 20261115]) {
      assert.deepEqual(check(feeds(pubDate)), [`latest.json has the publish date ${JSON.stringify(pubDate)}.`], String(pubDate));
    }
  });
});

// The workflow and the PowerShell build scripts have nothing to call, so
// these tests read their text.
const workflow = readFileSync(join(repositoryRoot, ".github/workflows/release.yml"), "utf8");
const job = (name) => {
  const match = workflow.match(new RegExp(`\\n  ${name}:\\n([\\s\\S]*?)(?=\\n  [a-z-]+:\\n|$)`));
  assert.ok(match, `job ${name}`);
  return match[1];
};
const readRepo = (path) => readFileSync(join(repositoryRoot, path), "utf8");

describe("release workflow", () => {
  it("releases every main push only after all tests, through a checked draft", () => {
    assert.match(workflow, /on:\n  push:\n    branches: \[main\]/);
    for (const name of ["build-windows", "build-android"]) {
      for (const line of [
        "needs: [plan, test-web, test-desktop, test-android]",
        "if: needs.plan.outputs.release == 'true'",
        "node scripts/release.mjs set-version",
      ]) {
        assert.ok(job(name).includes(line), `${name}: ${line}`);
      }
    }
    assert.match(job("publish"), /needs: \[plan, build-windows, build-android\][\s\S]*node scripts\/release\.mjs publish/);
    // publish() runs gh itself; the draft is checked before it is made latest.
    const steps = readRepo("scripts/release.mjs");
    const draft = steps.indexOf('"--draft", "--target"');
    const assetCheck = steps.indexOf("checkReleaseAssets({ tag_name: tag, assets: draft.assets }", draft);
    const check = steps.indexOf("checkFeeds({", draft);
    assert.ok(draft > -1 && assetCheck > draft && check > assetCheck && steps.indexOf('"--draft=false", "--latest"') > check);
  });

  it("hands only the versioned builds from the build jobs to publish", () => {
    // publish makes the fixed-name installer and APK, so the build jobs do not upload them.
    for (const [name, files] of [
      ["build-windows", ["_x64-setup.exe", "_x64-setup.exe.sig"]],
      ["build-android", [".apk", ".apk.sha256"]],
    ]) {
      const uploaded = job(name).match(/upload-artifact@v\d[\s\S]*?path: \|\n((?: {12}\S.*\n?)+)/);
      assert.ok(uploaded, name);
      const paths = uploaded[1].trim().split(/\n\s*/);
      assert.equal(paths.length, files.length, name);
      for (const [index, ending] of files.entries()) {
        assert.ok(paths[index].includes("${{ needs.plan.outputs.version }}") && paths[index].endsWith(ending), paths[index]);
      }
    }
  });

  it("runs the tests of every desktop Rust file that has them", () => {
    // The core crate declares the Rust files without Tauri once, for the app
    // and its tests; a file it leaves out has tests that never run, and
    // lib.rs cannot be built there at all.
    const sourceDir = join(repositoryRoot, "apps/desktop/src-tauri/src");
    const core = readRepo("apps/desktop/src-tauri/core/lib.rs");
    const app = readRepo("apps/desktop/src-tauri/src/lib.rs");
    // Files that call Win32 through the windows crate build only in the
    // Windows app, so their tests run there.
    const tested = readdirSync(sourceDir)
      .filter((name) => name.endsWith(".rs"))
      .map((name) => [name, readFileSync(join(sourceDir, name), "utf8")])
      .filter(([, source]) => source.includes("#[cfg(test)]") && !/^use windows::/m.test(source))
      .map(([name]) => name);
    assert.ok(tested.includes("transcription_sessions.rs"));
    assert.ok(!tested.includes("lib.rs"));
    assert.ok(!tested.includes("text_inserter.rs"));
    for (const name of tested) {
      assert.ok(core.includes(`#[path = "../src/${name}"]`), name);
      assert.ok(!app.includes(`mod ${name.replace(/\.rs$/, "")};`), name);
    }
    assert.match(job("test-desktop"), /working-directory: apps\/desktop\/src-tauri\s+run: cargo test --locked -p openwhisper_core\n/);
  });

  it("tests the core crate with the versions the app ships", () => {
    // The app and the core crate are one Cargo workspace with one Cargo.lock;
    // a second lock beside core let its tests drift to other reqwest and
    // rustls versions than the installer builds.
    assert.match(readRepo("apps/desktop/src-tauri/Cargo.toml"), /^\[workspace\]\nmembers = \["core"\]$/m);
    assert.ok(!existsSync(join(repositoryRoot, "apps/desktop/src-tauri/core/Cargo.lock")));
  });
});

describe("build scripts", () => {
  it("signs Windows builds without waiting for a key prompt", () => {
    // Fixed in 8ed08aa: an interactive password prompt hung unattended builds.
    const desktop = JSON.parse(readRepo("apps/desktop/package.json"));
    assert.match(desktop.scripts["tauri:build:signed"], /--ci\b/);
    assert.match(readRepo("scripts/build-windows-installer.ps1"), /npm run tauri:build:signed/);
  });

  it("refreshes the stable local installer after every Windows build", () => {
    // Fixed in 54acd04: only the release scripts refreshed it, so local builds left an old one.
    const build = readRepo("scripts/build-windows-installer.ps1");
    assert.ok(build.includes("Copy-Item -LiteralPath $VersionedInstaller -Destination $StableInstaller -Force"));
    assert.ok(build.includes('$OutDir = Join-Path $OriginalRepoRoot "artifacts/windows-installer"'));
  });

  it("never stops processes the Windows build did not start", () => {
    // Fixed in da44f50: the build stopped Node processes it had not started.
    assert.ok(!readRepo("scripts/build-windows-installer.ps1").includes("Stop-Process"));
  });

  it("keeps local Rust builds out of the folder Tauri watches", () => {
    // A target folder inside src-tauri restarts tauri dev on every build.
    assert.match(readRepo(".gitignore"), /^target\*\/\s*$/m);
    assert.ok(readRepo("scripts/run-local.ps1").includes('"target\\desktop-dev"'));
  });

  it("never seeds the repo-root .env with the placeholder signing key", () => {
    // The Windows installer loads that .env, so a copied placeholder replaced
    // a valid TAURI_SIGNING_PRIVATE_KEY already set in the environment.
    for (const launcher of ["scripts/run-local.ps1", "scripts/run-local.sh"]) {
      assert.ok(!readRepo(launcher).includes(".env.example"), launcher);
    }
    assert.ok(!readRepo("README.md").includes("cp .env.example .env"));
    assert.match(readRepo("scripts/load-env.ps1"), /\[Parameter\(Mandatory = \$true\)\]\s+\[string\]\$Path\s*\)/);
  });
});

describe("build caches", () => {

  it("builds the Windows installer in the Rust folder the cache keeps", () => {
    const windows = job("build-windows");
    const cached = windows.match(/Swatinem\/rust-cache@v2\s+with:\s+workspaces: (\S+) -> (\S+)/);
    assert.ok(cached, "build-windows caches its Rust build");
    const built = windows.match(/build-windows-installer\.ps1 -CargoTargetDir \(Join-Path \$env:GITHUB_WORKSPACE "([^"]+)"\)/);
    assert.ok(built, "build-windows passes its Rust folder to the build script");
    assert.equal(built[1], `${cached[1]}/${cached[2]}`);
    assert.ok(windows.indexOf("rust-cache") < windows.indexOf("build-windows-installer.ps1"));
  });

  it("downloads the Android toolchain again only when its script changes", () => {
    const android = job("build-android");
    assert.match(android, /actions\/cache@v\d[\s\S]*?\.tools[\s\S]*?hashFiles\('scripts\/bootstrap-android\.ps1'\)/);
    assert.ok(android.indexOf("actions/cache") < android.indexOf("./scripts/bootstrap-android.ps1"));
    assert.match(android, /gradle\/actions\/setup-gradle@/);
    assert.match(job("test-android"), /gradle\/actions\/setup-gradle@/);
  });

  it("caches npm downloads in every job that installs them", () => {
    for (const name of ["test-web", "test-desktop", "build-windows"]) {
      assert.match(job(name), /cache: npm\s+cache-dependency-path: apps\/(web|desktop)\/package-lock\.json/, name);
    }
    // The desktop calls OpenAI itself; no Python is set up or bundled.
    assert.doesNotMatch(workflow, /setup-uv|apps\/worker/);
  });

  it("gives plan and publish the full history and tags to check the release order", () => {
    for (const name of ["plan", "publish"]) {
      assert.match(job(name), /actions\/checkout@v4\s+with:\s+fetch-depth: 0\s+fetch-tags: true/, name);
    }
  });
});
