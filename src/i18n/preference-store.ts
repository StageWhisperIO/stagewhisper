import { isLanguagePreference, type LanguagePreference } from "./locale";

const PREFERENCE_KEY = "stagewhisper.ui-language";

type Listener = () => void;
const listeners = new Set<Listener>();

function readStorage(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeStorage(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    return;
  }
}

function notify(): void {
  for (const listener of listeners) listener();
}

export function readLanguagePreference(): LanguagePreference {
  const stored = readStorage(PREFERENCE_KEY);
  return isLanguagePreference(stored) ? stored : "system";
}

export function writeLanguagePreference(preference: LanguagePreference): void {
  if (readLanguagePreference() === preference) return;
  writeStorage(PREFERENCE_KEY, preference);
  notify();
}

export function readSystemLocales(): readonly string[] {
  if (typeof navigator === "undefined") return [];
  if (navigator.languages && navigator.languages.length > 0) return navigator.languages;
  return navigator.language ? [navigator.language] : [];
}

export function subscribeToLanguageInputs(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function onStorage(event: StorageEvent): void {
  if (event.key === PREFERENCE_KEY) notify();
}

if (typeof window !== "undefined") {
  window.addEventListener("storage", onStorage);
  window.addEventListener("languagechange", notify);
}
