import { invoke } from "@tauri-apps/api/core";
import { isIosLinuxKitBackend } from "./capabilities";

export type LinuxKitHealth = {
  available: boolean;
  version?: string;
  message?: string;
};

/**
 * Lightweight probe hook for future iOS-specific UI. Desktop builds report
 * unavailable without invoking anything. Once the Rust bridge is implemented,
 * add a `linuxkit_health` Tauri command and use this in startup diagnostics.
 */
export async function probeIosLinuxKit(): Promise<LinuxKitHealth> {
  if (!isIosLinuxKitBackend()) {
    return { available: false, message: "not running with ios-linuxkit backend" };
  }
  try {
    return await invoke<LinuxKitHealth>("linuxkit_health");
  } catch (e) {
    return { available: false, message: String(e) };
  }
}
