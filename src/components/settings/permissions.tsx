import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { usePermissions, type PermissionState } from "@/hooks/permissions";

function StatusBadge({ state }: { state: PermissionState }) {
  if (state === "granted") {
    return (
      <span className="inline-flex items-center rounded-full bg-success-bg px-2.5 py-0.5 text-xs font-medium text-success">
        Granted
      </span>
    );
  }
  if (state === "denied") {
    return (
      <span className="inline-flex items-center rounded-full bg-danger-bg px-2.5 py-0.5 text-xs font-medium text-danger">
        Denied
      </span>
    );
  }
  if (state === "undetermined") {
    return (
      <span className="inline-flex items-center rounded-full bg-sidebar-active px-2.5 py-0.5 text-xs font-medium text-muted">
        Not requested
      </span>
    );
  }
  return (
    <span className="inline-flex items-center rounded-full bg-sidebar-active px-2.5 py-0.5 text-xs font-medium text-faint">
      Unavailable
    </span>
  );
}

function ScreenRecordingRow() {
  const { statuses, refresh, requestScreenRecording, openScreenRecordingSettings } = usePermissions();
  const [pending, setPending] = useState(false);

  const state = statuses.screenRecording;

  const enable = useCallback(async () => {
    if (pending) return;
    setPending(true);
    try {
      await requestScreenRecording();
      await refresh();
    } finally {
      setPending(false);
    }
  }, [pending, requestScreenRecording, refresh]);

  return (
    <div className="flex items-center justify-between gap-4">
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-heading">Screen Recording &amp; System Audio</p>
        <p className="mt-0.5 text-xs text-muted">
          Required to hear the other side of your calls. We only use the audio, never your screen.
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <StatusBadge state={state} />
        {state === "undetermined" && (
          <button
            onClick={() => void enable()}
            disabled={pending}
            className="shrink-0 rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent disabled:opacity-40"
          >
            Enable
          </button>
        )}
        {state !== "unsupported" && (
          <button
            onClick={() => void openScreenRecordingSettings()}
            className="shrink-0 rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
          >
            Open System Settings
          </button>
        )}
      </div>
    </div>
  );
}

function MicrophoneRow() {
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
    <div>
      <div className="flex items-center justify-between gap-4">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-medium text-heading">Capture my microphone</p>
          <p className="mt-0.5 text-xs text-muted">
            Optional. Transcribe your own voice on-device and label it &ldquo;You&rdquo; in the
            transcript and summary. The audio stays on your Mac; only the transcript reaches your
            assistant, after the call.
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

export function PermissionsSection() {
  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="border-b border-rule px-6 py-5">
        <h2 className="text-base font-semibold text-heading">Permissions</h2>
        <p className="mt-0.5 text-sm text-muted">Permissions StageWhisper uses on macOS.</p>
      </header>

      <div className="settings-scroll flex-1 overflow-y-auto px-6 py-5 space-y-5">
        <ScreenRecordingRow />
        <div className="border-t border-rule pt-5">
          <MicrophoneRow />
        </div>
      </div>
    </div>
  );
}
