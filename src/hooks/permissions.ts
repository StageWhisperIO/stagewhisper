import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type PermissionState = "granted" | "denied" | "undetermined" | "unsupported";

export type PermissionsStatus = {
  microphone: PermissionState;
  screenRecording: PermissionState;
  systemAudio: PermissionState;
};

const DEFAULT_PERMISSIONS_STATUS: PermissionsStatus = {
  microphone: "undetermined",
  screenRecording: "undetermined",
  systemAudio: "undetermined",
};

export function usePermissions(): {
  statuses: PermissionsStatus;
  refresh: () => Promise<void>;
  requestMicrophone: () => Promise<PermissionState>;
  openMicrophoneSettings: () => Promise<void>;
  requestSystemAudio: () => Promise<PermissionState>;
  openSystemAudioSettings: () => Promise<void>;
} {
  const [statuses, setStatuses] = useState<PermissionsStatus>(DEFAULT_PERMISSIONS_STATUS);

  const refresh = useCallback(async () => {
    const result = await invoke<PermissionsStatus>("get_permissions_status");
    setStatuses(result);
  }, []);

  useEffect(() => {
    void refresh();

    const unlistenStatusChanged = listen<PermissionsStatus>(
      "permissions-status-changed",
      (event) => setStatuses(event.payload),
    );

    const handleFocus = () => void refresh();
    window.addEventListener("focus", handleFocus);

    return () => {
      void unlistenStatusChanged.then((unlisten) => unlisten());
      window.removeEventListener("focus", handleFocus);
    };
  }, [refresh]);

  const requestMicrophone = useCallback(
    async (): Promise<PermissionState> => invoke<PermissionState>("request_microphone_permission"),
    [],
  );

  const openMicrophoneSettings = useCallback(async (): Promise<void> => {
    await invoke("open_microphone_privacy_settings");
  }, []);

  const requestSystemAudio = useCallback(
    async (): Promise<PermissionState> => invoke<PermissionState>("request_system_audio_permission"),
    [],
  );

  const openSystemAudioSettings = useCallback(async (): Promise<void> => {
    await invoke("open_screen_recording_privacy_settings");
  }, []);

  return {
    statuses,
    refresh,
    requestMicrophone,
    openMicrophoneSettings,
    requestSystemAudio,
    openSystemAudioSettings,
  };
}
