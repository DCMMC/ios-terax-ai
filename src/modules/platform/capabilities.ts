import { platform } from "@tauri-apps/plugin-os";

export type RuntimeBackend = "desktop" | "ios-linuxkit";

const currentPlatform = platform();
const isIOS = currentPlatform === "ios";

/**
 * Coarse feature switches for desktop vs. iOS builds.
 *
 * iOS should use rcarmo/ios-linuxkit as the execution/workspace backend rather
 * than attempting to treat the iOS sandbox as a local POSIX host.
 */
export const platformCapabilities = {
  platform: currentPlatform,
  backend: (isIOS ? "ios-linuxkit" : "desktop") as RuntimeBackend,
  localHostShell: !isIOS,
  linuxGuestShell: isIOS,
  arbitraryHostFs: !isIOS,
  multipleWindows: !isIOS,
  autoUpdater: !isIOS,
} as const;

export function isIosLinuxKitBackend(): boolean {
  return platformCapabilities.backend === "ios-linuxkit";
}
