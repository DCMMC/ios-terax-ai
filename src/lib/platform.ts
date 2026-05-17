import { platform } from "@tauri-apps/plugin-os";

const PLATFORM = (() => {
  try {
    return platform();
  } catch {
    return "";
  }
})();

export const IS_MAC = PLATFORM === "macos";
export const IS_LINUX = PLATFORM === "linux";
export const IS_WINDOWS = PLATFORM === "windows";
export const IS_IOS = PLATFORM === "ios";
export const IS_IOS_WEBKIT =
  typeof navigator !== "undefined" &&
  (/iPad|iPhone|iPod/.test(navigator.userAgent) ||
    (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1));
export const IS_IOS_RUNTIME = IS_IOS || IS_IOS_WEBKIT;
export const IS_APPLE = IS_MAC || IS_IOS_RUNTIME;

/** Custom window controls (min/max/close) are rendered by us only on
 * non-macOS platforms — macOS keeps the native traffic lights via the
 * overlay title bar. */
export const USE_CUSTOM_WINDOW_CONTROLS =
  !IS_MAC && !IS_IOS_RUNTIME && PLATFORM !== "";

export const MOD_KEY = IS_APPLE ? "⌘" : "Ctrl";
/** KeyBinding property name for the platform's primary modifier. */
export const MOD_PROP: "meta" | "ctrl" = IS_APPLE ? "meta" : "ctrl";
export const CTRL_KEY = IS_APPLE ? "⌃" : "Ctrl";
export const ALT_KEY = IS_APPLE ? "⌥" : "Alt";
export const SHIFT_KEY = IS_APPLE ? "⇧" : "Shift";
export const TAB_KEY = IS_APPLE ? "⇥" : "Tab";
export const ENTER_KEY = IS_APPLE ? "↵" : "Enter";

export const KEY_SEP = IS_APPLE ? "" : "+";

export function fmtShortcut(...parts: string[]): string {
  return parts.join(KEY_SEP);
}
