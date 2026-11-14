import i18next from "i18next";
import { DEFAULT_LOCALE, formattingLocale, matchSupportedLocale } from "./locale";
import { readSystemLocales } from "./preference-store";

function activeLocale(): string {
  const language = matchSupportedLocale(i18next.language || DEFAULT_LOCALE) ?? DEFAULT_LOCALE;
  return formattingLocale(language, readSystemLocales());
}

export function formatDate(value: Date | number | string, options?: Intl.DateTimeFormatOptions): string {
  return new Intl.DateTimeFormat(activeLocale(), options).format(new Date(value));
}

export function formatTime(value: Date | number | string, options?: Intl.DateTimeFormatOptions): string {
  return formatDate(value, { hour: "numeric", minute: "2-digit", ...options });
}

export function formatNumber(value: number, options?: Intl.NumberFormatOptions): string {
  return new Intl.NumberFormat(activeLocale(), options).format(value);
}

export function formatRelativeTime(
  value: number,
  unit: Intl.RelativeTimeFormatUnit,
  options?: Intl.RelativeTimeFormatOptions,
): string {
  return new Intl.RelativeTimeFormat(activeLocale(), { numeric: "auto", ...options }).format(value, unit);
}
