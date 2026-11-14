import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { usePermissions, type PermissionState } from "@/hooks/permissions";
import { isWindows, platformContext, platformName } from "@/lib/platform";
import { useTranslation } from "@/i18n";

function StatusBadge({ state }: { state: PermissionState }) {
  const { t } = useTranslation("settings");
  if (state === "granted") {
    return (
      <span className="inline-flex items-center rounded-none bg-success-bg px-2.5 py-0.5 text-xs font-medium text-success">
        {t("permissions.status.granted")}
      </span>
    );
  }
  if (state === "denied") {
    return (
      <span className="inline-flex items-center rounded-none bg-danger-bg px-2.5 py-0.5 text-xs font-medium text-danger">
        {t("permissions.status.denied")}
      </span>
    );
  }
  if (state === "undetermined") {
    return (
      <span className="inline-flex items-center rounded-none bg-sidebar-active px-2.5 py-0.5 text-xs font-medium text-muted">
        {t("permissions.status.undetermined")}
      </span>
    );
  }
  return (
    <span className="inline-flex items-center rounded-none bg-sidebar-active px-2.5 py-0.5 text-xs font-medium text-faint">
      {t("permissions.status.unsupported")}
    </span>
  );
}

function SystemAudioRow() {
  const { t } = useTranslation("settings");
  const { statuses, refresh, requestSystemAudio, openSystemAudioSettings } = usePermissions();
  const [pending, setPending] = useState(false);

  const state = statuses.systemAudio;

  const enable = useCallback(async () => {
    if (pending) return;
    setPending(true);
    try {
      await requestSystemAudio();
      await refresh();
    } finally {
      setPending(false);
    }
  }, [pending, requestSystemAudio, refresh]);

  return (
    <div className="flex items-center justify-between gap-4">
      <div className="min-w-0 flex-1">
        <p className="text-sm font-medium text-heading">{t("permissions.systemAudio.title")}</p>
        <p className="mt-0.5 text-xs text-muted">
          {t("permissions.systemAudio.description")}
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
            {t("permissions.systemAudio.enable")}
          </button>
        )}
        {state !== "unsupported" && (
          <button
            onClick={() => void openSystemAudioSettings()}
            className="shrink-0 rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
          >
            {t("permissions.openSettings", { context: platformContext })}
          </button>
        )}
      </div>
    </div>
  );
}

function MicrophoneRow() {
  const { t } = useTranslation("settings");
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
          <p className="text-sm font-medium text-heading">{t("permissions.microphone.title")}</p>
          <p className="mt-0.5 text-xs text-muted">
            {t("permissions.microphone.description", { context: platformContext })}
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
          {t("permissions.microphone.denied", { context: platformContext })}
        </button>
      )}
    </div>
  );
}

export function PermissionsSection() {
  const { t } = useTranslation("settings");
  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="border-b border-rule px-6 py-5">
        <h2 className="text-base font-semibold text-heading">{t("permissions.title")}</h2>
        <p className="mt-0.5 text-sm text-muted">
          {t("permissions.description", { platform: platformName })}
        </p>
      </header>

      <div className="settings-scroll flex-1 overflow-y-auto px-6 py-5 space-y-5">
        {!isWindows && <SystemAudioRow />}
        <div className={isWindows ? "" : "border-t border-rule pt-5"}>
          <MicrophoneRow />
        </div>
      </div>
    </div>
  );
}
