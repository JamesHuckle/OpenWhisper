import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
  repositoryRoot,
  setVersion,
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
      "apps/desktop/src-tauri/core-tests/lib.rs",
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

  it("refuses feeds from another version or with the wrong checksum", () => {
    const windows = windowsFeed({ repository, version: "0.2.27", signature, pubDate: "2026-11-15T10:00:00Z" });
    const android = { ...androidFeed({ repository, version, sha256: apkSha256 }), sha256: "00".repeat(32) };
    const problems = checkFeeds({ repository, version, signature, apkSha256, windows, android });
    assert.ok(problems.some((problem) => problem.includes("latest.json reports 0.2.27")));
    assert.ok(problems.some((problem) => problem.includes("wrong APK checksum")));
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
    const check = steps.indexOf("checkFeeds({", draft);
    assert.ok(draft > -1 && check > draft && steps.indexOf('"--draft=false", "--latest"') > check);
  });

  it("runs the tests of every desktop Rust file that has them", () => {
    // core-tests builds the Rust files without Tauri; a file it leaves out
    // has tests that never run, and lib.rs cannot be built there at all.
    const sourceDir = join(repositoryRoot, "apps/desktop/src-tauri/src");
    const coreTests = readRepo("apps/desktop/src-tauri/core-tests/lib.rs");
    const tested = readdirSync(sourceDir)
      .filter((name) => name.endsWith(".rs"))
      .filter((name) => readFileSync(join(sourceDir, name), "utf8").includes("#[cfg(test)]"));
    assert.ok(tested.includes("transcription_sessions.rs"));
    assert.ok(!tested.includes("lib.rs"));
    for (const name of tested) {
      assert.ok(coreTests.includes(`#[path = "../src/${name}"]`), name);
    }
    assert.match(job("test-desktop"), /working-directory: apps\/desktop\/src-tauri\/core-tests\s+run: cargo test --locked/);
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
