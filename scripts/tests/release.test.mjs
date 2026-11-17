import assert from "node:assert/strict";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";

import {
  androidFeed,
  androidVersionCode,
  applyVersion,
  checkFeeds,
  committedVersion,
  isAppChange,
  latestTag,
  needsRelease,
  nextVersion,
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

describe("build caches", () => {
  const workflow = readFileSync(join(repositoryRoot, ".github/workflows/release.yml"), "utf8");
  const job = (name) => {
    const match = workflow.match(new RegExp(`\\n  ${name}:\\n([\\s\\S]*?)(?=\\n  [a-z-]+:\\n|$)`));
    assert.ok(match, `job ${name}`);
    return match[1];
  };

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
});
