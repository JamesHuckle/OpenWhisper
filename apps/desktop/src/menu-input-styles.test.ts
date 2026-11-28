// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// Parses the real stylesheet and checks the settings menu's API key input and
// both prompt boxes (Dictionary, Refine with AI) share one input style, only
// the prompt boxes resize, and all three light their border on focus.
const style = document.createElement("style");
style.textContent = readFileSync(resolve(import.meta.dirname, "styles.css"), "utf8");
document.head.append(style);

// Every declaration the stylesheet gives one selector, later rules winning,
// whether the selector has a rule of its own or shares a grouped rule.
const declarationsFor = (selector: string) => {
  const declarations: Record<string, string> = {};
  for (const rule of Array.from(style.sheet!.cssRules)) {
    if (!(rule instanceof CSSStyleRule)) continue;
    if (!rule.selectorText.split(",").some((part) => part.trim() === selector)) continue;
    for (const name of Array.from(rule.style)) declarations[name] = rule.style.getPropertyValue(name);
  }
  return declarations;
};

const sharedInputStyle = {
  display: "block",
  width: "calc(100% - 32px)",
  margin: "6px 16px",
  padding: "9px 11px",
  border: "1px solid var(--border)",
  "border-radius": "10px",
  background: "rgba(255, 255, 255, 0.03)",
  color: "var(--text)",
  "font-size": "12px",
  "font-family": '"Cascadia Mono", "SFMono-Regular", Consolas, monospace',
  "letter-spacing": "0.02em",
  outline: "none",
};

// jsdom also lists the longhands of each shorthand, so the checks below name the
// shorthands and compare the inputs' full lists with each other.
describe("settings menu input styles", () => {
  it("draws the API key input with the shared input style", () => {
    expect(declarationsFor(".menu-key-input")).toMatchObject(sharedInputStyle);
  });

  it("draws the prompt boxes like the API key input, resizable between height limits", () => {
    expect(declarationsFor(".menu-prompt-input")).toEqual({
      ...declarationsFor(".menu-key-input"),
      resize: "vertical",
      "min-height": "56px",
      "max-height": "120px",
      "line-height": "1.45",
    });
  });

  it("lights each input's border green while it has focus", () => {
    const keyFocus = declarationsFor(".menu-key-input:focus");
    expect(keyFocus).toMatchObject({ "border-color": "rgba(126, 240, 191, 0.4)" });
    expect(Object.keys(keyFocus).every((name) => /^border(-\w+)?-color$/.test(name))).toBe(true);
    expect(declarationsFor(".menu-prompt-input:focus")).toEqual(keyFocus);
  });
});
