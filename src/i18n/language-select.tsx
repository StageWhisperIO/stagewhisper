import { useTranslation } from "./index";
import { isLanguagePreference, SUPPORTED_LOCALES, type LanguagePreference } from "./locale";
import { useLanguagePreference } from "./use-language";

const OPTIONS: readonly LanguagePreference[] = ["system", ...SUPPORTED_LOCALES];

export function LanguageSelect({ className }: { className?: string }) {
  const { t } = useTranslation();
  const { preference, setPreference } = useLanguagePreference();
  return (
    <label className={className}>
      <span>{t("language.label")}</span>
      <select
        value={preference}
        onChange={(event) => {
          if (isLanguagePreference(event.target.value)) setPreference(event.target.value);
        }}
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
