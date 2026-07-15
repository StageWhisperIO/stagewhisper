import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";

const SESSION_STATE_EVENT = "session-state-changed";
const SESSION_START_ERROR_EVENT = "session-start-error";

export type SessionState = "stopped" | "listening";

const isSessionState = (value: unknown): value is SessionState =>
  value === "stopped" || value === "listening";

export function useSessionState() {
  const [listeningState, setSessionState] = useState<SessionState>("stopped");
  const [sessionStartError, setSessionStartError] = useState<string | null>(null);

  useEffect(() => {
    let unlistenState: UnlistenFn | null = null;
    let unlistenError: UnlistenFn | null = null;
    let cancelled = false;

    const setup = async () => {
      try {
        const state = await invoke<string>("get_session_state");
        if (!cancelled && isSessionState(state)) {
          setSessionState(state);
        }
      } catch {
        // ignore
      }

      try {
        unlistenState = await listen<string>(SESSION_STATE_EVENT, (event) => {
          if (cancelled || !isSessionState(event.payload)) {
            return;
          }
          setSessionState(event.payload);
        });
      } catch {
        // ignore
      }

      try {
        unlistenError = await listen<string>(SESSION_START_ERROR_EVENT, (event) => {
          if (!cancelled) {
            setSessionStartError(event.payload);
          }
        });
      } catch {
        // ignore
      }
    };

    setup();

    return () => {
      cancelled = true;
      if (unlistenState) {
        unlistenState();
      }
      if (unlistenError) {
        unlistenError();
      }
    };
  }, []);

  const toggleSessionState = useCallback(async () => {
    setSessionStartError(null);
    try {
      await invoke<boolean>("toggle_session_state");
      const updatedState = await invoke<string>("get_session_state");
      if (isSessionState(updatedState)) {
        setSessionState(updatedState);
        return updatedState;
      }
    } catch {
      // ignore
    }
    return listeningState;
  }, [listeningState]);

  const dismissSessionStartError = useCallback(() => {
    setSessionStartError(null);
  }, []);

  return {
    isListening: listeningState === "listening",
    listeningState,
    toggleSessionState,
    sessionStartError,
    dismissSessionStartError,
  };
}
