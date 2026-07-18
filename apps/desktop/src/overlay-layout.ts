export const COLLAPSED_WIDTH = 38;
export const COLLAPSED_HEIGHT = 14;
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
  const anchorOffsetY = (widgetHeight - COLLAPSED_HEIGHT) / 2;
  const bubbleVisible = input.settingsLoaded && !input.hasOpenaiApiKey;
  const bubbleStackHeight = bubbleVisible ? API_KEY_BUBBLE_STACK_HEIGHT : 0;

  if (!input.menuVisible) {
    return {
      width: Math.max(widgetWidth, bubbleVisible ? API_KEY_BUBBLE_WIDTH : widgetWidth),
      height: widgetHeight + bubbleStackHeight,
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
    height: widgetHeight + Math.max(bubbleStackHeight, menuStackHeight),
    anchorOffsetY,
    bubbleVisible,
  };
}
