export const SUPPORTED_LOCALES = ["en", "pl"] as const;
export type Locale = (typeof SUPPORTED_LOCALES)[number];
export type LanguagePreference = "system" | Locale;
export const DEFAULT_LOCALE: Locale = "en";

export function isLanguagePreference(value: unknown): value is LanguagePreference {
  return value === "system" || SUPPORTED_LOCALES.includes(value as Locale);
}

export function matchSupportedLocale(tag: string): Locale | null {
  const primary = tag.trim().toLowerCase().split(/[-_]/)[0];
  return SUPPORTED_LOCALES.find((locale) => locale === primary) ?? null;
}

export interface LocaleInputs {
  preference: LanguagePreference;
  systemLocales: readonly string[];
}

export function resolveLocale({ preference, systemLocales }: LocaleInputs): Locale {
  if (preference !== "system") return preference;
  for (const tag of systemLocales) {
    const match = matchSupportedLocale(tag);
    if (match) return match;
  }
  return DEFAULT_LOCALE;
}

export function formattingLocale(locale: Locale, systemLocales: readonly string[]): string {
  return systemLocales.find((tag) => matchSupportedLocale(tag) === locale) ?? locale;
}
