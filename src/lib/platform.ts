export const isWindows = navigator.userAgent.includes("Windows");

export const modKey = isWindows ? "Ctrl" : "⌘";

export const modShiftLabel = isWindows ? "Ctrl+Shift" : "⌘⇧";

export const thisMachine = isWindows ? "this computer" : "this Mac";

export const settingsAppName = isWindows ? "Windows Settings" : "System Settings";

export function shortcutLabel(key: string): string {
  return isWindows ? `${modShiftLabel}+${key}` : `${modKey}${key}`;
}

export function modShiftShortcut(key: string): string {
  return isWindows ? `${modShiftLabel}+${key}` : `${modShiftLabel}${key}`;
}
