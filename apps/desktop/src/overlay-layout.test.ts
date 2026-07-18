import { describe, expect, it } from "vitest";
import {
  API_KEY_BUBBLE_STACK_HEIGHT,
  API_KEY_BUBBLE_WIDTH,
  EXPANDED_HEIGHT,
  EXPANDED_WIDTH,
  WINDOW_BOTTOM_PADDING,
  getOverlayLayout,
} from "./overlay-layout";

describe("overlay startup layout", () => {
  it("reserves the fixed expanded pill footprint while settings load", () => {
    const pending = getOverlayLayout({
      expanded: false,
      settingsLoaded: false,
      hasOpenaiApiKey: false,
      menuVisible: false,
    });

    expect(pending).toMatchObject({
      width: EXPANDED_WIDTH,
      height: EXPANDED_HEIGHT + WINDOW_BOTTOM_PADDING,
      anchorOffsetY: WINDOW_BOTTOM_PADDING,
      bubbleVisible: false,
    });
  });

  it("does not resize when loading an existing API key", () => {
    const pending = getOverlayLayout({
      expanded: false,
      settingsLoaded: false,
      hasOpenaiApiKey: false,
      menuVisible: false,
    });
    const loaded = getOverlayLayout({
      expanded: false,
      settingsLoaded: true,
      hasOpenaiApiKey: true,
      menuVisible: false,
    });

    expect(loaded).toEqual(pending);
  });

  it("keeps a single window footprint across hover expand/collapse", () => {
    const collapsed = getOverlayLayout({
      expanded: false,
      settingsLoaded: true,
      hasOpenaiApiKey: true,
      menuVisible: false,
    });
    const expanded = getOverlayLayout({
      expanded: true,
      settingsLoaded: true,
      hasOpenaiApiKey: true,
      menuVisible: false,
    });

    // Same width/height/anchor for both states => no native resize or
    // reposition on hover, so the pill renders once and never jumps.
    expect(expanded).toEqual(collapsed);
  });

  it("shows onboarding only after settings confirm the key is missing", () => {
    expect(
      getOverlayLayout({
        expanded: false,
        settingsLoaded: true,
        hasOpenaiApiKey: false,
        menuVisible: false,
      }),
    ).toMatchObject({
      width: API_KEY_BUBBLE_WIDTH,
      height: EXPANDED_HEIGHT + API_KEY_BUBBLE_STACK_HEIGHT + WINDOW_BOTTOM_PADDING,
      bubbleVisible: true,
    });
  });
});
