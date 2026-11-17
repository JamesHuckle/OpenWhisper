#!/usr/bin/env node
// Release steps for the GitHub Actions release workflow
// (.github/workflows/release.yml). Every push to main runs it.
//
//   node scripts/release.mjs plan
//     Prints release=, version=, version_code= and tag= lines for
//     $GITHUB_OUTPUT. A release is due when app code changed since the last
//     release tag. The version is the last tag + 1, or the committed desktop
//     version when someone raised it by hand (for a minor or major release).
//
//   node scripts/release.mjs set-version <version>
//     Writes the version into every desktop and Android version file. The
//     build jobs run it on their own copy; versions are never committed.
//
//   node scripts/release.mjs publish --version <version> --assets <dir> --target <sha>
//     Uploads the signed Windows and Android files and both update feeds to a
//     draft release, checks the draft, then publishes it as latest and checks
//     the public feeds. The tag is created only when the draft is published.

import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const repositoryRoot = resolve(fileURLToPath(new URL("..", import.meta.url)));
export const productName = "OpenWhisper";

// ── Versions ──────────────────────────────────────────────────────────────────

export function parseVersion(text) {
  const match = /^v?(\d+)\.(\d+)\.(\d+)$/.exec(String(text).trim());
  return match ? match.slice(1).map(Number) : null;
}

function compareVersions(a, b) {
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index] - b[index];
  }
  return 0;
}

export function latestTag(tags) {
  let best = null;
  for (const tag of tags) {
    const version = parseVersion(tag);
    if (!version || !tag.startsWith("v")) continue;
    if (!best || compareVersions(version, best.version) > 0) best = { tag, version };
  }
  return best ? best.tag : null;
}

// The next release is the last tag + 1. A committed version above the last
// tag wins, so a minor or major release is one hand edit of the version files.
export function nextVersion(lastTag, committedVersion) {
  const committed = parseVersion(committedVersion);
  if (!committed) throw new Error(`Unreadable committed version: ${committedVersion}`);
  const last = lastTag ? parseVersion(lastTag) : null;
  if (!last || compareVersions(committed, last) > 0) return committed.join(".");
  return [last[0], last[1], last[2] + 1].join(".");
}

// Android needs a whole number that grows with every release. 0.2.28 → 2028.
export function androidVersionCode(version) {
  const parts = parseVersion(version);
  if (!parts) throw new Error(`Unreadable version: ${version}`);
  const [major, minor, patch] = parts;
  if (minor > 999 || patch > 999 || major > 2000) {
    throw new Error(`Version ${version} does not fit the Android versionCode scheme.`);
  }
  return major * 1_000_000 + minor * 1_000 + patch;
}

// ── What needs a release ──────────────────────────────────────────────────────

// Paths that end up inside the Windows installer or the Android APK.
const appPaths = [
  "apps/desktop/",
  "apps/android/",
  "packages/protocol/",
  "scripts/build-windows-installer.ps1",
  "scripts/build-android-release.ps1",
];

// Files under those paths that never reach users.
const nonAppPatterns = [
  /\.md$/i,
  /\.test\.[cm]?[jt]s$/,
  /(^|\/)tests?\//,
  /^apps\/android\/app\/src\/(test|androidTest)\//,
  /^apps\/desktop\/src-tauri\/core-tests\//,
];

export function isAppChange(path) {
  return appPaths.some((prefix) => path.startsWith(prefix)) &&
    !nonAppPatterns.some((pattern) => pattern.test(path));
}

export function needsRelease(lastTag, changedPaths) {
  return !lastTag || changedPaths.some(isAppChange);
}

// ── Version files ─────────────────────────────────────────────────────────────

export const versionFiles = {
  tauriConfig: "apps/desktop/src-tauri/tauri.conf.json",
  cargoToml: "apps/desktop/src-tauri/Cargo.toml",
  cargoLock: "apps/desktop/src-tauri/Cargo.lock",
  packageJson: "apps/desktop/package.json",
  packageLock: "apps/desktop/package-lock.json",
  androidGradle: "apps/android/app/build.gradle.kts",
};

function replaceExactly(text, pattern, replacement, count, description) {
  let replaced = 0;
  const result = text.replace(pattern, (...groups) => {
    if (replaced >= count) return groups[0];
    replaced += 1;
    return replacement(...groups);
  });
  if (replaced !== count) throw new Error(`Unable to set the version in ${description}.`);
  return result;
}

const setters = {
  tauriConfig: (text, version) =>
    replaceExactly(text, /("version"\s*:\s*")[^"]+(")/g, (_, a, b) => `${a}${version}${b}`, 1, versionFiles.tauriConfig),
  cargoToml: (text, version) =>
    replaceExactly(text, /^(version\s*=\s*")[^"]+(")/gm, (_, a, b) => `${a}${version}${b}`, 1, versionFiles.cargoToml),
  cargoLock: (text, version) =>
    replaceExactly(
      text,
      /(\[\[package\]\]\s+name\s*=\s*"openwhisper_desktop"\s+version\s*=\s*")[^"]+(")/g,
      (_, a, b) => `${a}${version}${b}`,
      1,
      versionFiles.cargoLock,
    ),
  packageJson: (text, version) =>
    replaceExactly(text, /("version"\s*:\s*")[^"]+(")/g, (_, a, b) => `${a}${version}${b}`, 1, versionFiles.packageJson),
  // The lock file names the app's version twice: at the top and for "".
  packageLock: (text, version) =>
    replaceExactly(text, /^(\s*"version"\s*:\s*")[^"]+(")/gm, (_, a, b) => `${a}${version}${b}`, 2, versionFiles.packageLock),
  androidGradle: (text, version, versionCode) =>
    replaceExactly(
      replaceExactly(text, /(versionCode\s*=\s*)\d+/g, (_, a) => `${a}${versionCode}`, 1, versionFiles.androidGradle),
      /(versionName\s*=\s*")[^"]+(")/g,
      (_, a, b) => `${a}${version}${b}`,
      1,
      versionFiles.androidGradle,
    ),
};

export function applyVersion(name, text, version) {
  return setters[name](text, version, androidVersionCode(version));
}

export function setVersion(root, version) {
  if (!parseVersion(version)) throw new Error(`Unreadable version: ${version}`);
  for (const [name, path] of Object.entries(versionFiles)) {
    const fullPath = join(root, path);
    writeFileSync(fullPath, applyVersion(name, readFileSync(fullPath, "utf8"), version));
  }
}

export function committedVersion(root) {
  return JSON.parse(readFileSync(join(root, versionFiles.tauriConfig), "utf8")).version;
}

// ── Release files and update feeds ────────────────────────────────────────────

export function releaseAssetNames(version) {
  return {
    windowsInstaller: `${productName}_${version}_x64-setup.exe`,
    windowsSignature: `${productName}_${version}_x64-setup.exe.sig`,
    windowsStableInstaller: `${productName}_x64-setup.exe`,
    windowsFeed: "latest.json",
    androidApk: `${productName}-Android-v${version}.apk`,
    androidStableApk: `${productName}-Android.apk`,
    androidChecksum: `${productName}-Android-v${version}.apk.sha256`,
    androidFeed: `${productName}-Android-update.json`,
  };
}

export function downloadUrl(repository, tag, name) {
  return `https://github.com/${repository}/releases/download/${tag}/${name}`;
}

// The feed the desktop updater reads (tauri.conf.json → plugins.updater).
export function windowsFeed({ repository, version, signature, pubDate }) {
  const tag = `v${version}`;
  return {
    version,
    notes: `${productName} ${version} is ready. Your settings and API key will be preserved.`,
    pub_date: pubDate,
    platforms: {
      "windows-x86_64": {
        signature: signature.trim(),
        url: downloadUrl(repository, tag, releaseAssetNames(version).windowsInstaller),
      },
    },
  };
}

// The feed the Android updater reads (AndroidUpdateClient).
export function androidFeed({ repository, version, sha256 }) {
  return {
    versionCode: androidVersionCode(version),
    versionName: version,
    apkUrl: downloadUrl(repository, `v${version}`, releaseAssetNames(version).androidStableApk),
    sha256,
    releaseNotes: "",
  };
}

export function androidReleaseNotes(repository, version) {
  const apkUrl = downloadUrl(repository, `v${version}`, releaseAssetNames(version).androidStableApk);
  return `## Android beta

Download **${productName}-Android.apk** to install ${productName} on Android 9 or newer. The app works alongside the selected keyboard and requires microphone and user-enabled accessibility permissions. Live transcription uses a personal OpenAI API key saved in the app. On Samsung devices, Auto Blocker may need to be disabled temporarily for installation.

**[Download the Android APK](${apkUrl})**

SHA-256 checksums are included in the release assets. This APK is signed with the persistent ${productName} Android release key so future versions can update it in place.
`;
}

// Returns a list of problems; an empty list means both feeds match the files.
export function checkFeeds({ repository, version, signature, apkSha256, windows, android }) {
  const problems = [];
  const names = releaseAssetNames(version);
  const tag = `v${version}`;
  const platform = windows?.platforms?.["windows-x86_64"];
  if (windows?.version !== version) problems.push(`latest.json reports ${windows?.version}, expected ${version}.`);
  if (platform?.url !== downloadUrl(repository, tag, names.windowsInstaller)) {
    problems.push(`latest.json points to ${platform?.url}.`);
  }
  if (platform?.signature !== signature.trim()) problems.push("latest.json has the wrong installer signature.");
  if (android?.versionName !== version) {
    problems.push(`${names.androidFeed} reports ${android?.versionName}, expected ${version}.`);
  }
  if (android?.versionCode !== androidVersionCode(version)) {
    problems.push(`${names.androidFeed} has versionCode ${android?.versionCode}.`);
  }
  if (android?.apkUrl !== downloadUrl(repository, tag, names.androidStableApk)) {
    problems.push(`${names.androidFeed} points to ${android?.apkUrl}.`);
  }
  if (android?.sha256 !== apkSha256) problems.push(`${names.androidFeed} has the wrong APK checksum.`);
  return problems;
}

// ── Commands ──────────────────────────────────────────────────────────────────

function git(...args) {
  return execFileSync("git", ["-C", repositoryRoot, ...args], { encoding: "utf8" }).trim();
}

function gh(...args) {
  return execFileSync("gh", args, { encoding: "utf8", stdio: ["ignore", "pipe", "inherit"] }).trim();
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function plan() {
  const tags = git("tag", "--list", "v*").split("\n").filter(Boolean);
  const lastTag = latestTag(tags);
  const changed = lastTag ? git("diff", "--name-only", lastTag, "HEAD").split("\n").filter(Boolean) : [];
  const release = needsRelease(lastTag, changed);
  const version = nextVersion(lastTag, committedVersion(repositoryRoot));
  const lines = [
    `release=${release}`,
    `version=${version}`,
    `version_code=${androidVersionCode(version)}`,
    `tag=v${version}`,
  ];
  console.log(lines.join("\n"));
  console.error(
    release
      ? `App code changed since ${lastTag ?? "the first commit"}: releasing v${version}.`
      : `No app code changed since ${lastTag}: no release.`,
  );
}

async function fetchWithRetry(url, attempts = 10) {
  for (let attempt = 1; ; attempt += 1) {
    try {
      const response = await fetch(url, { headers: { "Cache-Control": "no-cache" } });
      if (!response.ok) throw new Error(`${url} answered ${response.status}`);
      return Buffer.from(await response.arrayBuffer());
    } catch (error) {
      if (attempt >= attempts) throw error;
      await new Promise((done) => setTimeout(done, 3000));
    }
  }
}

function parseOptions(args) {
  const options = {};
  for (let index = 0; index < args.length; index += 2) {
    if (!args[index].startsWith("--")) throw new Error(`Unexpected argument ${args[index]}`);
    options[args[index].slice(2)] = args[index + 1];
  }
  return options;
}

async function publish(options) {
  const { version, assets, target } = options;
  const repository = options.repository ?? process.env.GITHUB_REPOSITORY;
  if (!parseVersion(version) || !assets || !target || !repository) {
    throw new Error("publish needs --version, --assets, --target and GITHUB_REPOSITORY.");
  }
  const tag = `v${version}`;
  const names = releaseAssetNames(version);
  const path = (name) => join(resolve(assets), name);

  for (const name of [
    names.windowsInstaller,
    names.windowsSignature,
    names.windowsStableInstaller,
    names.androidApk,
    names.androidStableApk,
    names.androidChecksum,
  ]) {
    if (!existsSync(path(name))) throw new Error(`Missing release file: ${name}`);
  }
  const apkSha256 = sha256(path(names.androidApk));
  if (sha256(path(names.androidStableApk)) !== apkSha256) throw new Error("The two Android APKs differ.");
  if (sha256(path(names.windowsStableInstaller)) !== sha256(path(names.windowsInstaller))) {
    throw new Error("The two Windows installers differ.");
  }
  const signature = readFileSync(path(names.windowsSignature), "utf8").trim();
  if (!signature) throw new Error("The Windows updater signature is empty.");

  writeFileSync(
    path(names.windowsFeed),
    JSON.stringify(windowsFeed({ repository, version, signature, pubDate: new Date().toISOString().replace(/\.\d+Z$/, "Z") })),
  );
  writeFileSync(path(names.androidFeed), JSON.stringify(androidFeed({ repository, version, sha256: apkSha256 })));
  const uploads = Object.values(names);

  // A failed earlier run can leave a draft behind; never touch a published one.
  const existing = JSON.parse(gh("api", `repos/${repository}/releases`, "--paginate", "--slurp"))
    .flat()
    .filter((release) => release.tag_name === tag);
  for (const release of existing) {
    if (!release.draft) throw new Error(`Release ${tag} is already published.`);
    console.log(`==> Removing the unfinished draft ${tag} (${release.id})`);
    gh("api", "-X", "DELETE", `repos/${repository}/releases/${release.id}`);
  }

  const notesDirectory = mkdtempSync(join(tmpdir(), "openwhisper-release-"));
  try {
    const notesPath = join(notesDirectory, "notes.md");
    writeFileSync(notesPath, androidReleaseNotes(repository, version));
    console.log(`==> Creating draft release ${tag} for ${target}`);
    gh("release", "create", tag, "--repo", repository, "--draft", "--target", target,
      "--title", `${productName} ${tag}`, "--generate-notes", "--notes-file", notesPath);
    console.log("==> Uploading release files to the draft");
    gh("release", "upload", tag, "--repo", repository, "--clobber", ...uploads.map(path));

    console.log("==> Checking the draft");
    const draft = JSON.parse(gh("release", "view", tag, "--repo", repository, "--json", "isDraft,assets"));
    if (!draft.isDraft) throw new Error(`Release ${tag} is no longer a draft.`);
    for (const name of uploads) {
      const asset = draft.assets.find((candidate) => candidate.name === name);
      if (!asset || asset.state !== "uploaded" || asset.size !== statSync(path(name)).size) {
        throw new Error(`The draft is missing a complete ${name}.`);
      }
    }
    const downloads = join(notesDirectory, "draft");
    gh("release", "download", tag, "--repo", repository, "--dir", downloads,
      "--pattern", names.windowsFeed, "--pattern", names.androidFeed);
    const draftProblems = checkFeeds({
      repository,
      version,
      signature,
      apkSha256,
      windows: JSON.parse(readFileSync(join(downloads, names.windowsFeed), "utf8")),
      android: JSON.parse(readFileSync(join(downloads, names.androidFeed), "utf8")),
    });
    if (draftProblems.length) throw new Error(`The draft's update feeds are wrong:\n${draftProblems.join("\n")}`);

    console.log(`==> Publishing ${tag} as the latest release`);
    gh("release", "edit", tag, "--repo", repository, "--draft=false", "--latest");
  } finally {
    rmSync(notesDirectory, { recursive: true, force: true });
  }

  console.log("==> Checking the public update feeds and APK");
  const latest = (name) => `https://github.com/${repository}/releases/latest/download/${name}`;
  const publicProblems = checkFeeds({
    repository,
    version,
    signature,
    apkSha256,
    windows: JSON.parse(await fetchWithRetry(latest(names.windowsFeed))),
    android: JSON.parse(await fetchWithRetry(latest(names.androidFeed))),
  });
  const publicApk = await fetchWithRetry(downloadUrl(repository, tag, names.androidStableApk));
  if (createHash("sha256").update(publicApk).digest("hex") !== apkSha256) {
    publicProblems.push("The public APK does not match the built APK.");
  }
  if (publicProblems.length) throw new Error(`The public release is wrong:\n${publicProblems.join("\n")}`);
  console.log(`${productName} ${tag} is live for Windows and Android users.`);
}

async function main([command, ...args]) {
  if (command === "plan") return plan();
  if (command === "set-version" && args.length === 1) {
    setVersion(repositoryRoot, args[0]);
    console.log(`Version files set to ${args[0]} (Android versionCode ${androidVersionCode(args[0])}).`);
    return;
  }
  if (command === "publish") return publish(parseOptions(args));
  throw new Error("Usage: node scripts/release.mjs plan | set-version <version> | publish --version <v> --assets <dir> --target <sha>");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}
