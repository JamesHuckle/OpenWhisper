#!/usr/bin/env node
/**
 * Removes unused icon files after `npm run icon`.
 * Keeps only: icon.svg, icon.ico, icon.png, 32x32.png (required for tray + installer)
 */
const fs = require("fs");
const path = require("path");

const iconsDir = path.join(__dirname, "../src-tauri/icons");

const toRemove = [
  "StoreLogo.png",
  "128x128.png",
  "128x128@2x.png",
  "icon.icns",
  ...fs.readdirSync(iconsDir).filter((f) => f.startsWith("Square")),
];
const dirsToRemove = ["ios", "android"];

for (const f of toRemove) {
  const p = path.join(iconsDir, f);
  try {
    fs.unlinkSync(p);
    console.log("Removed", f);
  } catch (e) {
    if (e.code !== "ENOENT") throw e;
  }
}
for (const d of dirsToRemove) {
  const p = path.join(iconsDir, d);
  try {
    fs.rmSync(p, { recursive: true });
    console.log("Removed", d + "/");
  } catch (e) {
    if (e.code !== "ENOENT") throw e;
  }
}
