import { useState } from "react";
import { useLocalLlm, type LlmModelInfo } from "../../hooks/local-llm";

export function LocalLlmSection({ embedded = false }: { embedded?: boolean }) {
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
        <p className="text-sm font-medium text-heading">Local AI model</p>
        <p className="mt-1 text-xs text-muted">
          Run an assistant entirely on this device. It works offline, and your data never leaves the machine.
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
        Bigger models tend to give better answers, but use more memory and take longer to download.
      </p>

      {downloading && (
        <div className="flex items-center gap-3 rounded-lg border border-accent px-4 py-3">
          <div className="min-w-0 flex-1">
            <p className="text-xs font-medium text-heading">Downloading…</p>
            <div className="mt-1.5 h-1.5 w-full overflow-hidden rounded-full bg-rule">
              <div
                className="h-full rounded-full bg-accent transition-all duration-300"
                style={{ width: `${filePercent}%` }}
              />
            </div>
            <p className="mt-1 truncate text-[10px] text-faint">
              {filePercent}%
              {downloadProgress
                ? ` · file ${downloadProgress.filesCompleted}/${downloadProgress.filesTotal} · ${downloadProgress.fileName}`
                : ""}
            </p>
          </div>
          <button
            onClick={() => void cancelDownload()}
            className="shrink-0 rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-danger hover:text-danger"
          >
            Cancel
          </button>
        </div>
      )}

      <div className="space-y-2 rounded-lg border border-rule-strong px-4 py-3">
        <p className="text-sm font-medium text-heading">Custom Hugging Face model</p>
        <p className="text-xs text-muted">
          Download any GGUF model from Hugging Face by repo id. Add an access token for gated or
          private repos.
        </p>
        <input
          value={customRepo}
          onChange={(e) => setCustomRepo(e.target.value)}
          placeholder="e.g. unsloth/Qwen3-4B-Instruct-GGUF"
          className="w-full rounded-md border border-rule-strong bg-transparent px-3 py-2 text-sm text-body outline-none focus:border-accent"
        />
        <input
          value={hfToken}
          onChange={(e) => setHfToken(e.target.value)}
          placeholder="Hugging Face access token (optional)"
          type="password"
          className="w-full rounded-md border border-rule-strong bg-transparent px-3 py-2 text-sm text-body outline-none focus:border-accent"
        />
        <div className="flex flex-wrap items-center gap-2">
          <button
            disabled={customRepo.trim().length === 0 || downloading}
            onClick={() => void downloadModel(customRepo.trim(), hfToken)}
            className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent disabled:opacity-40"
          >
            Download custom model
          </button>
          <button
            disabled={customRepo.trim().length === 0 || downloading}
            onClick={() => void useCachedModel(customRepo.trim())}
            className="px-1 text-xs font-medium text-muted transition hover:text-accent disabled:opacity-40"
          >
            Already downloaded it? Use the cached copy
          </button>
        </div>

        {!models.some((model) => model.id === selectedId) && (
          <div className="flex items-center justify-between rounded-md border border-accent px-3 py-2">
            <div className="min-w-0">
              <p className="truncate text-xs font-medium text-heading">{status.label}</p>
              <p className="text-[10px] text-faint">
                {status.ready ? "Installed · in use" : "Selected · download to use"}
              </p>
            </div>
            <button
              onClick={() => void deleteModel(selectedId)}
              className="ml-3 shrink-0 rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-danger hover:text-danger"
            >
              Delete
            </button>
          </div>
        )}
      </div>

      <div className="space-y-2 rounded-lg border border-rule-strong px-4 py-3">
        <p className="text-sm font-medium text-heading">Use a model from your computer</p>
        <p className="text-xs text-muted">
          Already have model files on disk, from huggingface-cli, LM Studio, or an earlier download?
          Point StageWhisper at the folder.
        </p>
        <button
          onClick={() => void useLocalFolder()}
          className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
        >
          Choose a folder…
        </button>
        <p className="text-[10px] leading-relaxed text-faint">
          Pick a folder with a .gguf file inside. Ollama models won't work here, since Ollama keeps
          them in its own format.
        </p>
      </div>

      {downloadError && <p className="text-xs text-danger">{downloadError}</p>}

      <p className="text-[10px] leading-relaxed text-faint">
        Local models run entirely on your device.
      </p>
    </div>
  );
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
              Recommended
            </span>
          )}
          {model.ramHintGb > 0 && (
            <span className="text-xs text-faint">~{model.ramHintGb} GB RAM</span>
          )}
        </div>
        <div className="mt-1 flex items-center gap-2">
          {ready ? (
            <span className="text-xs text-emerald-500">Installed</span>
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
            Delete
          </button>
        ) : (
          <button
            onClick={onDownload}
            disabled={busy}
            className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent disabled:opacity-40"
          >
            Download
          </button>
        )}
      </div>
    </div>
  );
}
