import { invoke } from "@tauri-apps/api/core";
import { IS_IOS } from "@/lib/platform";

export type SettingsTab =
  | "general"
  | "shortcuts"
  | "models"
  | "agents"
  | "about";

export async function openSettingsWindow(tab?: SettingsTab): Promise<void> {
  if (IS_IOS && typeof window !== "undefined") {
    window.dispatchEvent(
      new CustomEvent<SettingsTab>("terax:settings-open", {
        detail: tab ?? "general",
      }),
    );
    return;
  }
  await invoke("open_settings_window", { tab: tab ?? null });
}
