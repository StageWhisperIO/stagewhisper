import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";

export function useModelReady() {
  const [modelReady, setModelReady] = useState<boolean | null>(null);

  useEffect(() => {
    let cancelled = false;

    const refresh = async () => {
      try {
        const status = await invoke<{ ready: boolean }>("get_model_status");
        if (!cancelled) {
          setModelReady(status.ready);
        }
      } catch {
        if (!cancelled) {
          setModelReady(null);
        }
      }
    };

    void refresh();

    const unlisteners: Promise<UnlistenFn>[] = [
      listen("model-download-complete", () => void refresh()),
    ];

    return () => {
      cancelled = true;
      for (const p of unlisteners) {
        p.then((fn) => fn());
      }
    };
  }, []);

  return modelReady;
}
