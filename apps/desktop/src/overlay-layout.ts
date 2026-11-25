export const COLLAPSED_WIDTH = 38;
export const COLLAPSED_HEIGHT = 14;
/** Matches WINDOW_BOTTOM_PADDING_LOGICAL in src-tauri/src/overlay_geometry.rs. */
export const WINDOW_BOTTOM_PADDING = 2;
export const EXPANDED_WIDTH = 92;
export const EXPANDED_HEIGHT = 26;
export const MENU_GAP = 12;
export const API_KEY_BUBBLE_WIDTH = 188;
export const API_KEY_BUBBLE_STACK_HEIGHT = 44;

export type OverlayLayoutInput = {
  /** Width of the pill while it shows a short inline notice such as "No audio found". */
  noticeWidth?: number;
  settingsLoaded: boolean;
  hasOpenaiApiKey: boolean;
  menuVisible: boolean;
  menuWidth?: number;
  menuHeight?: number;
};

export type OverlayLayout = {
  width: number;
  height: number;
};

export function getOverlayLayout(input: OverlayLayoutInput): OverlayLayout {
  // The native OS window always reserves the *expanded* pill footprint. Hover
  // expand/collapse and recording/transcribing state changes then only alter
  // the CSS-rendered pill *inside* this fixed footprint — they never resize or
  // reposition the OS window. This is what makes the pill deterministic: it is
  // pinned to a single on-screen location and rendered once, with no
  // per-transition resize+reposition jitter (and no pointer-enter/leave
  // feedback loop, since the window never moves out from under the cursor).
  //
  // Only four things ever change the native window: the mic menu opening, the
  // onboarding bubble appearing, a brief inline notice widening the pill, or
  // the cursor moving to another monitor.
  const frameWidth = Math.max(EXPANDED_WIDTH, Math.ceil(input.noticeWidth ?? 0));
  const frameHeight = EXPANDED_HEIGHT;
  const bubbleVisible = input.settingsLoaded && !input.hasOpenaiApiKey;
  const bubbleStackHeight = bubbleVisible ? API_KEY_BUBBLE_STACK_HEIGHT : 0;

  if (!input.menuVisible) {
    return {
      width: Math.max(frameWidth, bubbleVisible ? API_KEY_BUBBLE_WIDTH : frameWidth),
      height: frameHeight + bubbleStackHeight + WINDOW_BOTTOM_PADDING,
    };
  }

  const menuStackHeight = MENU_GAP + Math.ceil(input.menuHeight ?? 0);
  return {
    width: Math.max(
      frameWidth,
      Math.ceil(input.menuWidth ?? 0),
      bubbleVisible ? API_KEY_BUBBLE_WIDTH : 0,
    ),
    height: frameHeight + Math.max(bubbleStackHeight, menuStackHeight) + WINDOW_BOTTOM_PADDING,
  };
}

export type OverlayVisibilityInput = {
  /** The user's "Show pill when idle" setting. */
  showIdlePill: boolean;
  /** No recording or transcription is running. */
  idle: boolean;
  menuOpen: boolean;
  /** The tray's "Show Pill" item revealed it for now. */
  manualReveal: boolean;
  /** A brief inline notice such as "No audio found" is showing. */
  noticeShowing: boolean;
};

/**
 * Whether the native window is shown. A user who hides the idle pill still
 * sees it while dictating, while the menu or a notice is open, or after
 * choosing "Show Pill" in the tray.
 */
export function isOverlayVisible(input: OverlayVisibilityInput): boolean {
  return (
    input.showIdlePill || !input.idle || input.menuOpen || input.manualReveal || input.noticeShowing
  );
}

/**
 * What the native overlay window is told: its size and visibility. The native
 * side pins its bottom edge WINDOW_BOTTOM_PADDING below the anchor.
 */
export type WindowLayout = {
  width: number;
  height: number;
  visible: boolean;
};

function sameWindowLayout(a: WindowLayout, b: WindowLayout): boolean {
  return a.width === b.width && a.height === b.height && a.visible === b.visible;
}

/**
 * Sends window layouts to the native side one at a time, in the order they
 * were asked for. A request matching the newest one is skipped unless forced,
 * or unless the native side rejected that newest one (Windows placement can
 * fail to settle), so the next matching request tries again. A failure of an
 * older request does not undo a newer one still waiting in the queue.
 */
export function createWindowLayoutQueue(applyNative: (layout: WindowLayout) => Promise<void>) {
  let newestRequested: WindowLayout | null = null;
  let newestRequestFailed = false;
  let queue: Promise<void> = Promise.resolve();

  return {
    /** The newest layout sent or waiting to be sent, or null before the first. */
    newestRequested: () => newestRequested,

    async apply(layout: WindowLayout, force = false): Promise<void> {
      if (
        !force &&
        !newestRequestFailed &&
        newestRequested !== null &&
        sameWindowLayout(newestRequested, layout)
      ) {
        return;
      }

      newestRequested = layout;
      newestRequestFailed = false;
      const run = queue.catch(() => {}).then(() => applyNative(layout));
      queue = run;
      try {
        await run;
      } catch (error) {
        if (newestRequested === layout) newestRequestFailed = true;
        throw error;
      }
    },
  };
}
