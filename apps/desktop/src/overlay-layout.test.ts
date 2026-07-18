import { describe, expect, it } from "vitest";
import {
  API_KEY_BUBBLE_STACK_HEIGHT,
  API_KEY_BUBBLE_WIDTH,
  COLLAPSED_HEIGHT,
  COLLAPSED_WIDTH,
  WINDOW_BOTTOM_PADDING,
  getOverlayLayout,
} from "./overlay-layout";

describe("overlay startup layout", () => {
  it("stays collapsed while persisted settings are loading", () => {
    const pending = getOverlayLayout({
      expanded: false,
      settingsLoaded: false,
      hasOpenaiApiKey: false,
      menuVisible: false,
    });

    expect(pending).toMatchObject({
      width: COLLAPSED_WIDTH,
      height: COLLAPSED_HEIGHT + WINDOW_BOTTOM_PADDING,
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
      height: COLLAPSED_HEIGHT + API_KEY_BUBBLE_STACK_HEIGHT + WINDOW_BOTTOM_PADDING,
      bubbleVisible: true,
    });
  });
});
