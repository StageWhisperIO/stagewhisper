import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";

const RELAY_SETTINGS_CHANGED_EVENT = "relay-settings-changed";

type RelaySettings = {
  relayUrl: string;
  relayToken: string;
  pairedVerified?: boolean;
};

export function useRelayConnected() {
  const [connected, setConnected] = useState<boolean | null>(null);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    const refresh = async () => {
      try {
        const settings = await invoke<RelaySettings>("get_relay_settings");
        if (!cancelled) {
          setConnected(
            settings.relayUrl.trim().length > 0 &&
              settings.relayToken.trim().length > 0 &&
              Boolean(settings.pairedVerified),
          );
        }
      } catch {
        if (!cancelled) setConnected(false);
      }
    };

    void refresh();

    listen<boolean>(RELAY_SETTINGS_CHANGED_EVENT, (event) => {
      if (!cancelled) setConnected(event.payload);
    })
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  return connected;
}
