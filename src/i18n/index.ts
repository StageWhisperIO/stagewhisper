import i18next from "i18next";
import { initReactI18next } from "react-i18next";
import { DEFAULT_LOCALE, resolveLocale, type Locale } from "./locale";
import {
  readLanguagePreference,
  readSystemLocales,
  subscribeToLanguageInputs,
} from "./preference-store";
import { buildResources } from "./resources";
import { pushLocaleToNative } from "./native-locale";

type Catalog = { default: Record<string, unknown> };
const catalogs = import.meta.glob<Catalog>("./locales/*/*.json", { eager: true });
const { resources, namespaces } = buildResources(catalogs);

type LocaleListener = (locale: Locale) => void;
const localeListeners = new Set<LocaleListener>();

export function currentResolvedLocale(): Locale {
  return resolveLocale({
    preference: readLanguagePreference(),
    systemLocales: readSystemLocales(),
  });
}

void i18next.use(initReactI18next).init({
  resources,
  ns: namespaces,
  defaultNS: "common",
  lng: currentResolvedLocale(),
  fallbackLng: DEFAULT_LOCALE,
  supportedLngs: Object.keys(resources),
  interpolation: { escapeValue: false },
  returnNull: false,
  initAsync: false,
});

let appliedLocale: Locale | null = null;

function applyLocale(locale: Locale): void {
  if (appliedLocale === locale) return;
  appliedLocale = locale;
  if (typeof document !== "undefined") document.documentElement.lang = locale;
  if (i18next.language !== locale) void i18next.changeLanguage(locale);
  for (const listener of localeListeners) listener(locale);
}

export function onLocaleApplied(listener: LocaleListener): () => void {
  localeListeners.add(listener);
  listener(i18next.language as Locale);
  return () => localeListeners.delete(listener);
}

export const catalogResources = resources;

localeListeners.add(pushLocaleToNative);
applyLocale(currentResolvedLocale());
subscribeToLanguageInputs(() => applyLocale(currentResolvedLocale()));

export const i18n = i18next;
export const t = i18next.t.bind(i18next);
export { useTranslation, Trans } from "react-i18next";
export type { Locale, LanguagePreference } from "./locale";
