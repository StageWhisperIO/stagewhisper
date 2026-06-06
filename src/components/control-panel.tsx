import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { useSessionState } from "../hooks/listening";
import { useRelayConnected } from "../hooks/connection";
import { useModelReady } from "../hooks/model-ready";

function DownloadIcon() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
      <polyline points="7 10 12 15 17 10" />
      <line x1="12" y1="15" x2="12" y2="3" />
    </svg>
  );
}

function LinkIcon() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
      <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
    </svg>
  );
}

function SettingsCogIcon() {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .33 1.82l.05.05a2 2 0 1 1-2.83 2.83l-.05-.05a1.7 1.7 0 0 0-1.82-.33 1.7 1.7 0 0 0-1 1.55V21a2 2 0 1 1-4 0v-.08a1.7 1.7 0 0 0-1-1.55 1.7 1.7 0 0 0-1.82.33l-.05.05a2 2 0 1 1-2.83-2.83l.05-.05a1.7 1.7 0 0 0 .33-1.82 1.7 1.7 0 0 0-1.55-1H3a2 2 0 1 1 0-4h.08a1.7 1.7 0 0 0 1.55-1 1.7 1.7 0 0 0-.33-1.82l-.05-.05a2 2 0 1 1 2.83-2.83l.05.05a1.7 1.7 0 0 0 1.82.33h.1a1.7 1.7 0 0 0 .9-1.5V3a2 2 0 1 1 4 0v.08a1.7 1.7 0 0 0 1 1.55 1.7 1.7 0 0 0 1.82-.33l.05-.05a2 2 0 1 1 2.83 2.83l-.05.05a1.7 1.7 0 0 0-.33 1.82v.1a1.7 1.7 0 0 0 1.5.9H21a2 2 0 1 1 0 4h-.08a1.7 1.7 0 0 0-1.55 1z" />
    </svg>
  );
}

export function ControlPanel() {
  const { listeningState, toggleSessionState, sessionStartError, dismissSessionStartError } =
    useSessionState();
  const connected = useRelayConnected();
  const modelReady = useModelReady();

  const isListening = listeningState === "listening";

  const listenLabel = isListening ? "Stop" : "Listen";

  const handleListenClick = useCallback(() => {
    void toggleSessionState();
  }, [toggleSessionState]);

  const openSettingsWindow = useCallback(async () => {
    try {
      await invoke("open_settings_window");
    } catch (error) {
      console.error("[ControlPanel] failed to open settings window", error);
    }
  }, []);

  const openModelSettings = useCallback(async () => {
    try {
      await invoke("open_settings_window");
      await emit("settings-navigate", { section: "model" });
    } catch (error) {
      console.error("[ControlPanel] failed to open model settings", error);
    }
  }, []);

  const needsModel = connected !== false && modelReady === false;

  return (
    <div
      className="flex h-full w-full select-none items-center justify-center gap-2.5 px-3"
      data-tauri-drag-region
    >
      {connected === false ? (
        <button
          onClick={openSettingsWindow}
          className="flex items-center gap-1.5 rounded-md px-3 py-1 text-xs font-semibold text-amber-300/90 backdrop-blur hover:bg-white/10 active:bg-white/15"
        >
          <LinkIcon />
          Connect AI
        </button>
      ) : needsModel ? (
        <button
          onClick={openModelSettings}
          className="flex items-center gap-1.5 rounded-md px-3 py-1 text-xs font-semibold text-amber-300/90 backdrop-blur hover:bg-white/10 active:bg-white/15"
        >
          <DownloadIcon />
          Download model
        </button>
      ) : (
        <button
          onClick={handleListenClick}
          className="flex items-center gap-1.5 rounded-md px-3 py-1 text-xs font-semibold text-white/90 backdrop-blur hover:bg-white/10 active:bg-white/15"
        >
          {isListening ? (
            <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <rect x="4" y="4" width="16" height="16" rx="2" />
            </svg>
          ) : (
            <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
              <path d="M2 12h4l3-9 6 18 3-9h4" />
            </svg>
          )}
          {listenLabel}
        </button>
      )}

      {sessionStartError ? (
        <div className="flex items-center gap-1.5 rounded-md border border-red-400/30 bg-red-400/15 px-2.5 py-1 text-[11px] text-red-100">
          <span className="line-clamp-2">{sessionStartError}</span>
          <button
            onClick={dismissSessionStartError}
            className="shrink-0 rounded px-1 py-0.5 text-[10px] text-red-200/60 hover:text-red-100"
          >
            ✕
          </button>
        </div>
      ) : (
        <span className="flex items-center gap-1 text-[11px] text-white/45">
          On/Off-screen
          <kbd className="rounded bg-white/10 px-1 py-0.5 text-[10px] font-medium text-white/55">
            ⌘\
          </kbd>
        </span>
      )}

      <button
        onClick={openSettingsWindow}
        aria-label="Open settings"
        title="Open settings"
        className="relative rounded-md px-2 py-1 text-sm font-semibold text-white/80 backdrop-blur hover:bg-white/10 active:bg-white/15"
      >
        <SettingsCogIcon />
      </button>
    </div>
  );
}
