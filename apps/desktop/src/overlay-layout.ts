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
  const widgetWidth = input.expanded ? EXPANDED_WIDTH : COLLAPSED_WIDTH;
  const widgetHeight = input.expanded ? EXPANDED_HEIGHT : COLLAPSED_HEIGHT;
  // Keep the rendered widget at the same anchor while leaving enough native
  // window space for its anti-aliased bottom border on mixed-DPI monitors.
  const anchorOffsetY = (widgetHeight - COLLAPSED_HEIGHT) / 2 + WINDOW_BOTTOM_PADDING;
  const bubbleVisible = input.settingsLoaded && !input.hasOpenaiApiKey;
  const bubbleStackHeight = bubbleVisible ? API_KEY_BUBBLE_STACK_HEIGHT : 0;

  if (!input.menuVisible) {
    return {
      width: Math.max(widgetWidth, bubbleVisible ? API_KEY_BUBBLE_WIDTH : widgetWidth),
      height: widgetHeight + bubbleStackHeight + WINDOW_BOTTOM_PADDING,
      anchorOffsetY,
      bubbleVisible,
    };
  }

  const menuStackHeight = MENU_GAP + Math.ceil(input.menuHeight ?? 0);
  return {
    width: Math.max(
      widgetWidth,
      Math.ceil(input.menuWidth ?? 0),
      bubbleVisible ? API_KEY_BUBBLE_WIDTH : 0,
    ),
    height: widgetHeight + Math.max(bubbleStackHeight, menuStackHeight) + WINDOW_BOTTOM_PADDING,
    anchorOffsetY,
    bubbleVisible,
  };
}
