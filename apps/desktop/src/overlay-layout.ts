export const COLLAPSED_WIDTH = 38;
export const COLLAPSED_HEIGHT = 14;
export const WINDOW_BOTTOM_PADDING = 2;
export const EXPANDED_WIDTH = 92;
export const EXPANDED_HEIGHT = 26;
export const MENU_GAP = 12;
export const API_KEY_BUBBLE_WIDTH = 188;
export const API_KEY_BUBBLE_STACK_HEIGHT = 44;

export type OverlayLayoutInput = {
  expanded: boolean;
  settingsLoaded: boolean;
  hasOpenaiApiKey: boolean;
  menuVisible: boolean;
  menuWidth?: number;
  menuHeight?: number;
};

export type OverlayLayout = {
  width: number;
  height: number;
  anchorOffsetY: number;
  bubbleVisible: boolean;
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
  // Only three things ever change the native window: the mic menu opening, the
  // onboarding bubble appearing, or the cursor moving to another monitor.
  const frameWidth = EXPANDED_WIDTH;
  const frameHeight = EXPANDED_HEIGHT;
  // A constant anchor offset keeps the window's pinned bottom edge fixed across
  // every state. Combined with the CSS pill being bottom-anchored, the pill's
  // bottom edge never drifts; expansion grows upward from that fixed edge.
  const anchorOffsetY = WINDOW_BOTTOM_PADDING;
  const bubbleVisible = input.settingsLoaded && !input.hasOpenaiApiKey;
  const bubbleStackHeight = bubbleVisible ? API_KEY_BUBBLE_STACK_HEIGHT : 0;

  if (!input.menuVisible) {
    return {
      width: Math.max(frameWidth, bubbleVisible ? API_KEY_BUBBLE_WIDTH : frameWidth),
      height: frameHeight + bubbleStackHeight + WINDOW_BOTTOM_PADDING,
      anchorOffsetY,
      bubbleVisible,
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
    anchorOffsetY,
    bubbleVisible,
  };
}
