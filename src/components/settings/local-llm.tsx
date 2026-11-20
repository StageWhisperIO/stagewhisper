import { useState } from "react";
import { useLocalLlm, type LlmModelInfo } from "../../hooks/local-llm";
import { useTranslation } from "@/i18n";
import { formatNumber } from "@/i18n/format";

export function LocalLlmSection({ embedded = false }: { embedded?: boolean }) {
  const { t } = useTranslation("models");
  const {
    status,
    models,
    downloading,
    downloadProgress,
    downloadError,
    downloadModel,
    cancelDownload,
    deleteModel,
    selectModel,
    useLocalFolder,
    useCachedModel,
  } = useLocalLlm();

  const [customRepo, setCustomRepo] = useState("");
  const [hfToken, setHfToken] = useState("");

  if (!status) return null;

  const selectedId = status.selectedId;

  const filePercent =
    downloadProgress && downloadProgress.bytesTotal > 0
      ? Math.min(
          Math.round((downloadProgress.bytesDownloaded / downloadProgress.bytesTotal) * 100),
          100,
        )
      : 0;

  return (
    <div className={embedded ? "space-y-5" : "space-y-5 px-6 py-5"}>
      <div>
        <p className="text-sm font-medium text-heading">{t("llm.title")}</p>
        <p className="mt-1 text-xs text-muted">
          {t("llm.description")}
        </p>
      </div>

      <div className="space-y-2">
        {models.map((model) => (
          <ModelRow
            key={model.id}
            model={model}
            selected={model.id === selectedId}
            ready={model.id === selectedId && status.ready}
            busy={downloading}
            onSelect={() => void selectModel(model.id)}
            onDownload={() => void downloadModel(model.id)}
            onDelete={() => void deleteModel(model.id)}
          />
        ))}
      </div>

      <p className="text-[10px] leading-relaxed text-faint">
        {t("llm.sizeHint")}
      </p>

      {downloading && (
        <div className="flex items-center gap-3 rounded-lg border border-accent px-4 py-3">
          <div className="min-w-0 flex-1">
            <p className="text-xs font-medium text-heading">{t("llm.downloading")}</p>
            <div className="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-rule">
              <div
                className="h-full rounded-full bg-accent transition-all duration-300"
                style={{ width: `${filePercent}%` }}
              />
            </div>
            <p className="mt-1 truncate text-[10px] text-faint">
              {downloadProgress
                ? t("llm.progressDetail", {
                    percent: formatPercent(filePercent),
                    completed: downloadProgress.filesCompleted,
                    files: downloadProgress.filesTotal,
                    file: downloadProgress.fileName,
                  })
                : formatPercent(filePercent)}
            </p>
          </div>
          <button
            onClick={() => void cancelDownload()}
            className="shrink-0 rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-danger hover:text-danger"
          >
            {t("common:actions.cancel")}
          </button>
        </div>
      )}

      <div className="space-y-2 rounded-lg border border-rule-strong px-4 py-3">
        <p className="text-sm font-medium text-heading">{t("llm.custom.title")}</p>
        <p className="text-xs text-muted">
          {t("llm.custom.description")}
        </p>
        <input
          value={customRepo}
          onChange={(e) => setCustomRepo(e.target.value)}
          placeholder={t("llm.custom.repoPlaceholder")}
          className="w-full rounded-md border border-rule-strong bg-transparent px-3 py-2 text-sm text-body outline-none focus:border-accent"
        />
        <input
          value={hfToken}
          onChange={(e) => setHfToken(e.target.value)}
          placeholder={t("llm.custom.tokenPlaceholder")}
          type="password"
          className="w-full rounded-md border border-rule-strong bg-transparent px-3 py-2 text-sm text-body outline-none focus:border-accent"
        />
        <div className="flex flex-wrap items-center gap-2">
          <button
            disabled={customRepo.trim().length === 0 || downloading}
            onClick={() => void downloadModel(customRepo.trim(), hfToken)}
            className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent disabled:opacity-40"
          >
            {t("llm.custom.download")}
          </button>
          <button
            disabled={customRepo.trim().length === 0 || downloading}
            onClick={() => void useCachedModel(customRepo.trim())}
            className="px-1 text-xs font-medium text-muted transition hover:text-accent disabled:opacity-40"
          >
            {t("llm.custom.useCached")}
          </button>
        </div>

        {!models.some((model) => model.id === selectedId) && (
          <div className="flex items-center justify-between rounded-md border border-accent px-3 py-2">
            <div className="min-w-0">
              <p className="truncate text-xs font-medium text-heading">{status.label}</p>
              <p className="text-[10px] text-faint">
                {status.ready ? t("llm.custom.installed") : t("llm.custom.selected")}
              </p>
            </div>
            <button
              onClick={() => void deleteModel(selectedId)}
              className="ml-3 shrink-0 rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-danger hover:text-danger"
            >
              {t("common:actions.delete")}
            </button>
          </div>
        )}
      </div>

      <div className="space-y-2 rounded-lg border border-rule-strong px-4 py-3">
        <p className="text-sm font-medium text-heading">{t("llm.folder.title")}</p>
        <p className="text-xs text-muted">
          {t("llm.folder.description")}
        </p>
        <button
          onClick={() => void useLocalFolder()}
          className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
        >
          {t("llm.folder.choose")}
        </button>
        <p className="text-[10px] leading-relaxed text-faint">
          {t("llm.folder.hint")}
        </p>
      </div>

      {downloadError && <p className="text-xs text-danger">{downloadError}</p>}

      <p className="text-[10px] leading-relaxed text-faint">
        {t("llm.footer")}
      </p>
    </div>
  );
}

function formatPercent(percent: number): string {
  return formatNumber(percent / 100, { style: "percent" });
}

interface ModelRowProps {
  model: LlmModelInfo;
  selected: boolean;
  ready: boolean;
  busy: boolean;
  onSelect: () => void;
  onDownload: () => void;
  onDelete: () => void;
}

function ModelRow({ model, selected, ready, busy, onSelect, onDownload, onDelete }: ModelRowProps) {
  const { t } = useTranslation("models");
  return (
    <div
      className={`flex items-center justify-between rounded-lg border px-4 py-3 transition ${
        selected ? "border-accent" : "border-rule-strong"
      }`}
    >
      <button onClick={onSelect} className="min-w-0 flex-1 text-left">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-heading">{model.label}</span>
          {model.recommended && (
            <span className="rounded-none bg-accent/10 px-2 py-0.5 text-[10px] font-medium text-accent">
              {t("llm.row.recommended")}
            </span>
          )}
          {model.ramHintGb > 0 && (
            <span className="text-xs text-faint">{t("llm.row.ram", { size: formatNumber(model.ramHintGb) })}</span>
          )}
        </div>
        <div className="mt-1 flex items-center gap-2">
          {ready ? (
            <span className="text-xs text-emerald-500">{t("llm.row.installed")}</span>
          ) : (
            <span className="text-xs text-muted">{model.repoId}</span>
          )}
        </div>
      </button>

      <div className="ml-3 flex shrink-0 items-center gap-2">
        {ready ? (
          <button
            onClick={onDelete}
            className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-danger hover:text-danger"
          >
            {t("common:actions.delete")}
          </button>
        ) : (
          <button
            onClick={onDownload}
            disabled={busy}
            className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent disabled:opacity-40"
          >
            {t("llm.row.download")}
          </button>
        )}
      </div>
    </div>
  );
}
