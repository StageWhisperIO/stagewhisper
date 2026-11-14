import { invoke } from "@tauri-apps/api/core";
import type { Locale } from "./locale";

function insideTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

export function pushLocaleToNative(locale: Locale): void {
  if (!insideTauri()) return;
  invoke("set_ui_locale", { locale }).catch((error: unknown) => {
    console.warn("[i18n] native locale sync failed:", error instanceof Error ? error.message : error);
  });
}
