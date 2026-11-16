import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const layout = await readFile(new URL("../app/layout.tsx", import.meta.url), "utf8");
const css = await readFile(new URL("../app/globals.css", import.meta.url), "utf8");

test("the site serves Inter itself, so text never waits on Google's font servers", () => {
  assert.match(layout, /import \{ Inter \} from "next\/font\/google";/);
  assert.match(layout, /variable: "--font-inter"/);
  assert.match(layout, /className=\{`dark \$\{inter\.variable\}`\}/);
  assert.doesNotMatch(layout, /fonts\.(googleapis|gstatic)\.com/);
  assert.match(css, /--font-sans: var\(--font-inter\), ui-sans-serif/);
});
