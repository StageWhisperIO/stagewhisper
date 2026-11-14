import { useTranslation } from "@/i18n";
import { isLanguagePreference, SUPPORTED_LOCALES, type LanguagePreference } from "@/i18n/locale";
import { useLanguagePreference } from "@/i18n/use-language";

const OPTIONS: readonly LanguagePreference[] = ["system", ...SUPPORTED_LOCALES];

export function LanguagePicker() {
  const { t } = useTranslation();
  const { preference, setPreference } = useLanguagePreference();
  return (
    <label className="mb-3 flex items-center justify-between gap-2">
      <span className="text-xs font-medium text-muted">{t("language.label")}</span>
      <select
        value={preference}
        onChange={(event) => {
          if (isLanguagePreference(event.target.value)) setPreference(event.target.value);
        }}
        title={t("language.description")}
        className="min-w-0 max-w-[8.5rem] truncate rounded-lg border border-rule-strong bg-page px-2 py-1.5 text-xs font-medium text-body outline-none transition hover:border-accent focus:border-accent"
      >
        {OPTIONS.map((option) => (
          <option key={option} value={option} lang={option === "system" ? undefined : option}>
            {t(`language.${option}`)}
          </option>
        ))}
      </select>
    </label>
  );
}
