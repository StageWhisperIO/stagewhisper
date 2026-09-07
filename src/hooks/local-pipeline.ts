import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { userFacingError } from "@/lib/utils";

interface ModelStatus {
  ready: boolean;
  exists: boolean;
  modelDir: string;
}

interface DownloadProgress {
  fileName: string;
  bytesDownloaded: number;
  bytesTotal: number;
  filesCompleted: number;
  filesTotal: number;
}

export type { ModelStatus, DownloadProgress };

export function useLocalPipeline() {
  const [modelStatus, setModelStatus] = useState<ModelStatus>({
    ready: false,
    exists: false,
    modelDir: "",
  });
  const [pipelineMode, setPipelineMode] = useState<"cloud" | "local">("cloud");
  const [pipelineLoading, setPipelineLoading] = useState(false);
  const [downloading, setDownloading] = useState(false);
  const [downloadProgress, setDownloadProgress] = useState<DownloadProgress | null>(null);
  const [downloadError, setDownloadError] = useState<string | null>(null);
  const [droppedChunks, setDroppedChunks] = useState(0);
  const [loadError, setLoadError] = useState<string | null>(null);

  const refreshStatus = useCallback(async () => {
    try {
      const status = await invoke<ModelStatus>("get_model_status");
      setModelStatus(status);
      const mode = await invoke<string>("get_pipeline_mode");
      setPipelineMode(mode as "cloud" | "local");
    } catch (e) {
      console.error("[useLocalPipeline] failed to refresh status:", e);
    }
  }, []);

  useEffect(() => {
    void refreshStatus();

    const unlisteners: Promise<UnlistenFn>[] = [];

    unlisteners.push(
      listen<DownloadProgress>("model-download-progress", (event) => {
        setDownloadProgress(event.payload);
      }),
    );

    unlisteners.push(
      listen("model-download-complete", () => {
        setDownloading(false);
        setDownloadProgress(null);
        setDownloadError(null);
        void refreshStatus();
      }),
    );

    unlisteners.push(
      listen<string>("model-download-error", (event) => {
        setDownloading(false);
        setDownloadError(
          userFacingError(event.payload, "Couldn't download the speech model. Check your connection and try again."),
        );
      }),
    );

    unlisteners.push(
      listen<string>("pipeline-mode-changed", (event) => {
        setPipelineMode(event.payload as "cloud" | "local");
      }),
    );

    unlisteners.push(
      listen<boolean>("pipeline-loading", (event) => {
        setPipelineLoading(event.payload);
        if (event.payload) {
          setDroppedChunks(0);
          setLoadError(null);
        }
      }),
    );

    unlisteners.push(
      listen<number>("pipeline-audio-dropped", (event) => {
        setDroppedChunks(event.payload);
      }),
    );

    unlisteners.push(
      listen<string>("pipeline-load-error", (event) => {
        setLoadError(userFacingError(event.payload, "Couldn't start local processing. Try again."));
      }),
    );

    return () => {
      for (const p of unlisteners) {
        p.then((fn) => fn());
      }
    };
  }, [refreshStatus]);

  const downloadModels = useCallback(async () => {
    setDownloading(true);
    setDownloadError(null);
    setDownloadProgress(null);
    try {
      await invoke("download_models");
    } catch (e) {
      setDownloading(false);
      setDownloadError(
        userFacingError(e, "Couldn't download the speech model. Check your connection and try again."),
      );
    }
  }, []);

  const resetPipelineErrors = useCallback(() => {
    setDroppedChunks(0);
    setLoadError(null);
  }, []);

  return {
    modelStatus,
    pipelineMode,
    pipelineLoading,
    downloading,
    downloadProgress,
    downloadError,
    droppedChunks,
    loadError,
    resetPipelineErrors,
    downloadModels,
    refreshStatus,
  };
}
