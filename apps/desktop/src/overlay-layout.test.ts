import { describe, expect, it } from "vitest";
import {
  API_KEY_BUBBLE_STACK_HEIGHT,
  API_KEY_BUBBLE_WIDTH,
  EXPANDED_HEIGHT,
  EXPANDED_WIDTH,
  WINDOW_BOTTOM_PADDING,
  type WindowLayout,
  createWindowLayoutQueue,
  getOverlayLayout,
  isOverlayVisible,
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

  it("widens only for an inline notice and keeps the pinned bottom edge", () => {
    const pill = getOverlayLayout({
      expanded: true,
      settingsLoaded: true,
      hasOpenaiApiKey: true,
      menuVisible: false,
    });
    const notice = getOverlayLayout({
      expanded: true,
      noticeWidth: 131.4,
      settingsLoaded: true,
      hasOpenaiApiKey: true,
      menuVisible: false,
    });

    expect(notice).toEqual({ ...pill, width: 132 });
  });
});

describe("overlay visibility", () => {
  const hiddenIdle = {
    showIdlePill: false,
    idle: true,
    menuOpen: false,
    manualReveal: false,
    noticeShowing: false,
  };

  it("hides the idle pill completely when the user turns it off", () => {
    expect(isOverlayVisible(hiddenIdle)).toBe(false);
    expect(isOverlayVisible({ ...hiddenIdle, showIdlePill: true })).toBe(true);
  });

  it("still shows a hidden pill while dictating, in the menu, after a notice or a tray reveal", () => {
    expect(isOverlayVisible({ ...hiddenIdle, idle: false })).toBe(true);
    expect(isOverlayVisible({ ...hiddenIdle, menuOpen: true })).toBe(true);
    expect(isOverlayVisible({ ...hiddenIdle, noticeShowing: true })).toBe(true);
    expect(isOverlayVisible({ ...hiddenIdle, manualReveal: true })).toBe(true);
  });
});

describe("window layout queue", () => {
  const shown: WindowLayout = { width: 92, height: 28, anchorOffsetY: 2, visible: true };
  const withMenu: WindowLayout = { width: 200, height: 180, anchorOffsetY: 2, visible: true };

  function recordingNative(failFirst = 0) {
    const calls: WindowLayout[] = [];
    let failuresLeft = failFirst;
    const applyNative = async (layout: WindowLayout) => {
      calls.push(layout);
      if (failuresLeft > 0) {
        failuresLeft -= 1;
        throw new Error("Initial overlay placement did not settle");
      }
    };
    return { calls, applyNative };
  }

  it("skips a request matching the newest one unless forced", async () => {
    const native = recordingNative();
    const queue = createWindowLayoutQueue(native.applyNative);

    await queue.apply(shown);
    await queue.apply({ ...shown });
    expect(native.calls).toHaveLength(1);

    await queue.apply({ ...shown }, true);
    expect(native.calls).toHaveLength(2);
    expect(queue.newestRequested()).toEqual(shown);
  });

  it("retries the same layout after the native side rejected it", async () => {
    const native = recordingNative(1);
    const queue = createWindowLayoutQueue(native.applyNative);

    await expect(queue.apply(shown)).rejects.toThrow("did not settle");
    await queue.apply({ ...shown });
    expect(native.calls).toEqual([shown, shown]);

    await queue.apply({ ...shown });
    expect(native.calls).toHaveLength(2);
  });

  it("keeps a newer queued layout when an older one fails", async () => {
    const native = recordingNative(1);
    const queue = createWindowLayoutQueue(native.applyNative);

    const first = queue.apply(shown);
    const second = queue.apply(withMenu);
    await expect(first).rejects.toThrow("did not settle");
    await second;
    expect(native.calls).toEqual([shown, withMenu]);

    await queue.apply({ ...withMenu });
    expect(native.calls).toHaveLength(2);
    expect(queue.newestRequested()).toEqual(withMenu);
  });

  it("sends layouts one at a time in the order they were asked for", async () => {
    const order: string[] = [];
    let releaseFirst!: () => void;
    let firstStarted!: () => void;
    const started = new Promise<void>((resolve) => (firstStarted = resolve));
    const queue = createWindowLayoutQueue(async (layout) => {
      order.push(`start ${layout.width}`);
      if (layout === shown) {
        firstStarted();
        await new Promise<void>((resolve) => (releaseFirst = resolve));
      }
      order.push(`end ${layout.width}`);
    });

    const first = queue.apply(shown);
    const second = queue.apply(withMenu);
    await started;
    releaseFirst();
    await Promise.all([first, second]);
    expect(order).toEqual(["start 92", "end 92", "start 200", "end 200"]);
  });
});
