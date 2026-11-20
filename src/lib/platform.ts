export const isWindows = navigator.userAgent.includes("Windows");

export const modKey = isWindows ? "Ctrl" : "⌘";

export const modShiftLabel = isWindows ? "Ctrl+Shift" : "⌘⇧";

export const platformContext = isWindows ? "windows" : "mac";

export const platformName = isWindows ? "Windows" : "macOS";

export function shortcutLabel(key: string): string {
  return isWindows ? `${modShiftLabel}+${key}` : `${modKey}${key}`;
}

export function modShiftShortcut(key: string): string {
  return isWindows ? `${modShiftLabel}+${key}` : `${modShiftLabel}${key}`;
}
