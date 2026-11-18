import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

const CURRENT_SESSION_EVENT = "current-session-changed";

export function useCurrentSessionId(): string | null {
  const [sessionId, setSessionId] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;

    invoke<string | null>("get_current_session_id")
      .then((value) => {
        if (!cancelled) setSessionId(value ?? null);
      })
      .catch((err) => {
        console.error("[useCurrentSessionId] failed to fetch session id:", err);
      });

    const window = getCurrentWebviewWindow();
    const unlistenPromise = window.listen<string | null>(CURRENT_SESSION_EVENT, (event) => {
      setSessionId(event.payload ?? null);
    });

    return () => {
      cancelled = true;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  return sessionId;
}
