import { useCallback, useSyncExternalStore } from "react";
import { useTranslation } from "./index";
import type { LanguagePreference, Locale } from "./locale";
import {
  readLanguagePreference,
  subscribeToLanguageInputs,
  writeLanguagePreference,
} from "./preference-store";

export function useLanguagePreference() {
  const { i18n } = useTranslation();
  const preference = useSyncExternalStore(subscribeToLanguageInputs, readLanguagePreference);
  const setPreference = useCallback((next: LanguagePreference) => writeLanguagePreference(next), []);
  return {
    preference,
    setPreference,
    locale: i18n.language as Locale,
  };
}
