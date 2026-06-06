import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useLocalPipeline } from "@/hooks/local-pipeline";
import { usePermissions } from "@/hooks/permissions";

function MicrophoneSection() {
  const { statuses, requestMicrophone, openMicrophoneSettings, refresh } = usePermissions();
  const [micEnabled, setMicEnabled] = useState(false);

  useEffect(() => {
    void invoke<{ micEnabled: boolean }>("get_app_settings").then(({ micEnabled }) =>
      setMicEnabled(micEnabled),
    );
  }, []);

  const denied = statuses.microphone === "denied";

  const toggle = useCallback(async () => {
    if (micEnabled) {
      await invoke("save_app_settings", { args: { micEnabled: false } });
      setMicEnabled(false);
      return;
    }
    const state = await requestMicrophone();
    await refresh();
    if (state === "granted") {
      await invoke("save_app_settings", { args: { micEnabled: true } });
      setMicEnabled(true);
    }
  }, [micEnabled, requestMicrophone, refresh]);

  return (
    <div className="border-t border-rule pt-5">
      <div className="flex items-center justify-between gap-4">
        <div>
          <p className="text-sm font-medium text-heading">Capture my microphone</p>
          <p className="mt-1 text-xs text-muted">
            Transcribe your own voice on-device and label it &ldquo;You&rdquo; in the transcript and
            summary. The audio stays on your Mac; only the transcript reaches your assistant, after
            the call.
          </p>
        </div>
        <button
          role="switch"
          aria-checked={micEnabled}
          onClick={() => void toggle()}
          className={`relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors ${
            micEnabled ? "bg-accent" : "bg-sidebar-active"
          }`}
        >
          <span
            className={`inline-block h-5 w-5 transform rounded-full bg-white shadow transition-transform ${
              micEnabled ? "translate-x-[22px]" : "translate-x-0.5"
            }`}
          />
        </button>
      </div>
      {denied && (
        <button
          onClick={() => void openMicrophoneSettings()}
          className="mt-2 text-xs text-accent underline"
        >
          Microphone access denied. Open System Settings to grant it.
        </button>
      )}
    </div>
  );
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function LocalPipelineSection() {
  const { modelStatus, downloading, downloadProgress, downloadError, downloadModels } =
    useLocalPipeline();

  const progressPercent =
    downloadProgress && downloadProgress.bytesTotal > 0
      ? Math.round((downloadProgress.bytesDownloaded / downloadProgress.bytesTotal) * 100)
      : 0;

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="border-b border-rule px-6 py-5">
        <h2 className="text-base font-semibold text-heading">Model</h2>
        <p className="mt-0.5 text-sm text-muted">
          On-device transcription. Audio never leaves your machine.
        </p>
      </header>

      <div className="settings-scroll flex-1 overflow-y-auto px-6 py-5 space-y-5">
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
              <p className="text-sm font-medium text-heading">Model ready</p>
              <p className="mt-0.5 text-xs text-muted">
                On-device transcription is active. Audio stays local.
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
              <p className="text-sm font-medium text-heading">Downloading model…</p>
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
                    {formatBytes(downloadProgress.bytesDownloaded)} /{" "}
                    {formatBytes(downloadProgress.bytesTotal)}
                    {" · "}
                    File {downloadProgress.filesCompleted + 1} of {downloadProgress.filesTotal}
                  </span>
                </div>
              </>
            )}
          </div>
        ) : (
          <div className="space-y-3">
            <div>
              <p className="text-sm font-medium text-heading">Model not downloaded</p>
              <p className="mt-1 text-xs text-muted">
                Download the on-device transcription model to start transcribing locally (~1.2 GB).
              </p>
            </div>
            <button
              onClick={() => void downloadModels()}
              className="rounded-lg bg-accent px-5 py-2.5 text-sm font-medium text-accent-fg transition hover:bg-accent-hover"
            >
              Download model
            </button>
          </div>
        )}

        {downloadError && (
          <div className="rounded-lg bg-danger-bg px-4 py-3">
            <p className="text-sm text-danger">{downloadError}</p>
          </div>
        )}

        <div className="border-t border-rule pt-5">
          <p className="text-sm font-medium text-heading">Storage</p>
          <p className="mt-1 text-xs text-muted">~1.2 GB total for the transcription model.</p>
          {modelStatus.modelDir && (
            <p className="mt-1.5 break-all rounded-lg bg-sidebar-active px-3 py-2 font-mono text-xs text-faint">
              {modelStatus.modelDir}
            </p>
          )}
        </div>

        <MicrophoneSection />
      </div>
    </div>
  );
}
