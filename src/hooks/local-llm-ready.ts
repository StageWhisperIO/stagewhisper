import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";

export function useLocalLlmReady() {
  const [ready, setReady] = useState<boolean | null>(null);
  const [prefersLocal, setPrefersLocal] = useState(false);

  useEffect(() => {
    let cancelled = false;

    const refresh = async () => {
      try {
        const status = await invoke<{ ready: boolean; primaryResponder: string }>(
          "get_local_llm_status",
        );
        if (!cancelled) {
          setReady(status.ready);
          setPrefersLocal(status.primaryResponder === "local");
        }
      } catch {
        if (!cancelled) {
          setReady(null);
          setPrefersLocal(false);
        }
      }
    };

    void refresh();

    const unlisteners: Promise<UnlistenFn>[] = [
      listen("local-llm-download-complete", () => void refresh()),
      listen("local-llm-download-cancelled", () => void refresh()),
      listen("local-llm-status-changed", () => void refresh()),
      listen("responder-preference-changed", () => void refresh()),
    ];

    return () => {
      cancelled = true;
      for (const p of unlisteners) {
        p.then((fn) => fn());
      }
    };
  }, []);

  return { ready, prefersLocal };
}
