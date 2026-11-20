import { useLocalPipeline } from "@/hooks/local-pipeline";
import { t as translate, useTranslation } from "@/i18n";
import { formatNumber } from "@/i18n/format";

const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;

function fixed(value: number, digits: number): string {
  return formatNumber(value, { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

function formatBytes(bytes: number): string {
  if (bytes < KB) return translate("models:units.bytes", { value: formatNumber(bytes) });
  if (bytes < MB) return translate("models:units.kb", { value: fixed(bytes / KB, 0) });
  if (bytes < GB) return translate("models:units.mb", { value: fixed(bytes / MB, 1) });
  return translate("models:units.gb", { value: fixed(bytes / GB, 2) });
}

export function LocalPipelineSection({ embedded = false }: { embedded?: boolean }) {
  const { t } = useTranslation("models");
  const { modelStatus, downloading, downloadProgress, downloadError, downloadModels } =
    useLocalPipeline();

  const progressPercent =
    downloadProgress && downloadProgress.bytesTotal > 0
      ? Math.round((downloadProgress.bytesDownloaded / downloadProgress.bytesTotal) * 100)
      : 0;

  const body = (
    <div className="space-y-5">
      {modelStatus.ready ? (
        <div className="flex items-start gap-3 rounded-lg bg-success-bg px-4 py-3">
          <svg
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.5"
            strokeLinecap="round"
            strokeLinejoin="round"
            className="mt-0.5 shrink-0 text-success"
          >
            <polyline points="20 6 9 17 4 12" />
          </svg>
          <div>
            <p className="text-sm font-medium text-heading">{t("speech.ready")}</p>
            <p className="mt-0.5 text-xs text-muted">
              {t("speech.readyDescription")}
            </p>
          </div>
        </div>
      ) : downloading ? (
        <div className="space-y-3">
          <div className="flex items-center gap-2">
            <svg className="h-4 w-4 animate-spin text-accent" viewBox="0 0 16 16" fill="none">
              <circle
                cx="8"
                cy="8"
                r="6"
                stroke="currentColor"
                strokeWidth="2"
                strokeDasharray="28"
                strokeDashoffset="8"
                strokeLinecap="round"
              />
            </svg>
            <p className="text-sm font-medium text-heading">{t("speech.downloading")}</p>
          </div>

          {downloadProgress && (
            <>
              <div className="h-1.5 w-full overflow-hidden rounded-full bg-sidebar-active">
                <div
                  className="h-full rounded-full bg-accent transition-all duration-300"
                  style={{ width: `${progressPercent}%` }}
                />
              </div>
              <div className="flex items-center justify-between text-xs text-muted">
                <span>{downloadProgress.fileName}</span>
                <span>
                  {t("speech.progress", {
                    downloaded: formatBytes(downloadProgress.bytesDownloaded),
                    total: formatBytes(downloadProgress.bytesTotal),
                    current: downloadProgress.filesCompleted + 1,
                    files: downloadProgress.filesTotal,
                  })}
                </span>
              </div>
            </>
          )}
        </div>
      ) : (
        <div className="space-y-3">
          <div>
            <p className="text-sm font-medium text-heading">{t("speech.missing")}</p>
            <p className="mt-1 text-xs text-muted">
              {t("speech.missingDescription")}
            </p>
          </div>
          <button
            onClick={() => void downloadModels()}
            className="rounded-lg bg-accent px-5 py-2.5 text-sm font-medium text-accent-fg transition hover:bg-accent-hover"
          >
            {t("speech.download")}
          </button>
        </div>
      )}

      {downloadError && (
        <div className="rounded-lg bg-danger-bg px-4 py-3">
          <p className="text-sm text-danger">{downloadError}</p>
        </div>
      )}

      <div className="border-t border-rule pt-5">
        <p className="text-sm font-medium text-heading">{t("speech.storage")}</p>
        <p className="mt-1 text-xs text-muted">{t("speech.storageDescription")}</p>
        {modelStatus.modelDir && (
          <p className="mt-1.5 break-all rounded-lg bg-sidebar-active px-3 py-2 font-mono text-xs text-faint">
            {modelStatus.modelDir}
          </p>
        )}
      </div>
    </div>
  );

  if (embedded) {
    return body;
  }

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="border-b border-rule px-6 py-5">
        <h2 className="text-base font-semibold text-heading">{t("speech.title")}</h2>
        <p className="mt-0.5 text-sm text-muted">
          {t("speech.description")}
        </p>
      </header>
      <div className="settings-scroll flex-1 overflow-y-auto px-6 py-5">{body}</div>
    </div>
  );
}
