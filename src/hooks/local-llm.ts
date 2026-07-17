import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface LlmModelInfo {
  id: string;
  repoId: string;
  label: string;
  ramHintGb: number;
  recommended: boolean;
  kind: string;
}

export interface LlmStatus {
  ready: boolean;
  exists: boolean;
  modelDir: string;
  selectedId: string;
  label: string;
  primaryResponder: string;
}

export interface LlmDownloadProgress {
  fileName: string;
  bytesDownloaded: number;
  bytesTotal: number;
  filesCompleted: number;
  filesTotal: number;
}

export function useLocalLlm() {
  const [status, setStatus] = useState<LlmStatus | null>(null);
  const [models, setModels] = useState<LlmModelInfo[]>([]);
  const [downloading, setDownloading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<LlmDownloadProgress | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextModels] = await Promise.all([
        invoke<LlmStatus>("get_local_llm_status"),
        invoke<LlmModelInfo[]>("list_local_llm_models"),
      ]);
      setStatus(nextStatus);
      setModels(nextModels);
    } catch (err) {
      console.error("[useLocalLlm] refresh failed", err);
    }
  }, []);

  useEffect(() => {
    void refresh();

    let dead = false;
    const cleanups: UnlistenFn[] = [];

    listen<LlmDownloadProgress>("local-llm-download-progress", (e) => {
      if (dead) return;
      setDownloading(true);
      setDownloadProgress(e.payload);
    }).then((fn) => { if (dead) fn(); else cleanups.push(fn); });

    listen("local-llm-download-complete", () => {
      if (dead) return;
      setDownloading(false);
      setDownloadProgress(null);
      setDownloadError(null);
      void refresh();
    }).then((fn) => { if (dead) fn(); else cleanups.push(fn); });

    listen<string>("local-llm-download-error", (e) => {
      if (dead) return;
      setDownloading(false);
      setDownloadProgress(null);
      setDownloadError(e.payload);
      void refresh();
    }).then((fn) => { if (dead) fn(); else cleanups.push(fn); });

    listen("local-llm-download-cancelled", () => {
      if (dead) return;
      setDownloading(false);
      setDownloadProgress(null);
      setDownloadError(null);
      void refresh();
    }).then((fn) => { if (dead) fn(); else cleanups.push(fn); });

    listen("responder-preference-changed", () => {
      if (dead) return;
      void refresh();
    }).then((fn) => { if (dead) fn(); else cleanups.push(fn); });

    return () => {
      dead = true;
      cleanups.forEach((fn) => fn());
    };
  }, [refresh]);

  const downloadModel = useCallback(
    async (modelIdOrRepo: string, hfToken?: string) => {
      setDownloading(true);
      setDownloadError(null);
      setDownloadProgress(null);
      try {
        await invoke("download_local_llm_model", {
          modelIdOrRepo,
          hfToken: hfToken && hfToken.trim().length > 0 ? hfToken.trim() : null,
        });
      } catch (err) {
        setDownloading(false);
        setDownloadError(err instanceof Error ? err.message : String(err));
      }
    },
    [],
  );

  const cancelDownload = useCallback(async () => {
    try {
      await invoke("cancel_local_llm_download");
    } catch (err) {
      console.error("[useLocalLlm] cancel failed", err);
    }
  }, []);

  const deleteModel = useCallback(
    async (modelIdOrRepo: string) => {
      try {
        await invoke("delete_local_llm_model", { modelIdOrRepo });
        await refresh();
      } catch (err) {
        setDownloadError(err instanceof Error ? err.message : String(err));
      }
    },
    [refresh],
  );

  const selectModel = useCallback(
    async (modelIdOrRepo: string) => {
      try {
        await invoke("set_local_llm_model", { modelIdOrRepo });
        await refresh();
      } catch (err) {
        setDownloadError(err instanceof Error ? err.message : String(err));
      }
    },
    [refresh],
  );

  const setPrimary = useCallback(
    async (preference: "external" | "local") => {
      try {
        await invoke("set_responder_preference", { preference });
        await refresh();
      } catch (err) {
        setDownloadError(err instanceof Error ? err.message : String(err));
      }
    },
    [refresh],
  );

  const useLocalFolder = useCallback(async () => {
    setDownloadError(null);
    try {
      const next = await invoke<LlmStatus | null>("use_local_llm_folder");
      if (next) await refresh();
      return next;
    } catch (err) {
      setDownloadError(err instanceof Error ? err.message : String(err));
      return null;
    }
  }, [refresh]);

  const useCachedModel = useCallback(
    async (repoId: string) => {
      setDownloadError(null);
      try {
        const next = await invoke<LlmStatus | null>("use_hf_cache_model", { repoId });
        if (next) await refresh();
        else setDownloadError(`Couldn't find ${repoId} in your Hugging Face cache.`);
        return next;
      } catch (err) {
        setDownloadError(err instanceof Error ? err.message : String(err));
        return null;
      }
    },
    [refresh],
  );

  return {
    status,
    models,
    downloading,
    downloadProgress,
    downloadError,
    refresh,
    downloadModel,
    cancelDownload,
    deleteModel,
    selectModel,
    setPrimary,
    useLocalFolder,
    useCachedModel,
  };
}
