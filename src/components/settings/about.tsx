import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { useTranslation } from "@/i18n";

export function AboutSection() {
  const { t } = useTranslation("settings");
  const [appVersion, setAppVersion] = useState<string | null>(null);

  useEffect(() => {
    getVersion()
      .then(setAppVersion)
      .catch(() => {});
  }, []);

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="border-b border-rule px-6 py-5">
        <h2 className="text-base font-semibold text-heading">{t("about.title")}</h2>
        <p className="mt-0.5 text-sm text-muted">StageWhisper Free</p>
      </header>

      <div className="settings-scroll flex-1 overflow-y-auto px-6 py-5 space-y-5">
        <section className="space-y-3">
          <p className="text-sm leading-relaxed text-body">
            {t("about.intro")}
          </p>
          <p className="text-sm leading-relaxed text-body">
            {t("about.pitch")}
          </p>
        </section>

        <div className="border-t border-rule pt-5 space-y-3">
          <div>
            <p className="text-sm font-medium text-heading">{t("about.howItWorks.title")}</p>
            <p className="mt-1 text-xs leading-relaxed text-muted">
              {t("about.howItWorks.body")}
            </p>
          </div>
        </div>

        <div className="border-t border-rule pt-5 space-y-3">
          <div>
            <p className="text-sm font-medium text-heading">{t("about.ownership.title")}</p>
            <p className="mt-1 text-xs leading-relaxed text-muted">
              {t("about.ownership.body")}
            </p>
          </div>
        </div>

        <div className="border-t border-rule pt-5 space-y-3">
          <div>
            <p className="text-sm font-medium text-heading">{t("about.privacy.title")}</p>
            <p className="mt-1 text-xs leading-relaxed text-muted">
              {t("about.privacy.body")}
            </p>
          </div>
        </div>
      </div>

      {appVersion && (
        <div className="border-t border-rule px-6 py-3">
          <p className="text-xs text-faint">{t("about.version", { version: appVersion })}</p>
        </div>
      )}
    </div>
  );
}
