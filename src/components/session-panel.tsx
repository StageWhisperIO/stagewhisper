import { useCallback, useEffect, useRef, useState } from "react";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { useMessagesFeed } from "../hooks/messages";
import { useSessionState } from "../hooks/listening";
import { useLocalPipeline } from "../hooks/local-pipeline";
import { TranscriptView } from "./transcript";

const VAD_STATUS_EVENT = "vad-status";

function useListeningTimer(isListening: boolean) {
  const [elapsed, setElapsed] = useState(0);
  const startTimeRef = useRef<number | null>(null);
  const rafRef = useRef<number | null>(null);

  useEffect(() => {
    if (isListening) {
      startTimeRef.current = Date.now();
      const tick = () => {
        if (startTimeRef.current !== null) {
          setElapsed(Math.floor((Date.now() - startTimeRef.current) / 1000));
        }
        rafRef.current = requestAnimationFrame(tick);
      };
      rafRef.current = requestAnimationFrame(tick);
    } else {
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
      startTimeRef.current = null;
      setElapsed(0);
    }

    return () => {
      if (rafRef.current !== null) {
        cancelAnimationFrame(rafRef.current);
        rafRef.current = null;
      }
    };
  }, [isListening]);

  const minutes = String(Math.floor(elapsed / 60)).padStart(2, "0");
  const seconds = String(elapsed % 60).padStart(2, "0");

  return `${minutes}:${seconds}`;
}

export function SessionPanel() {
  const { listeningState } = useSessionState();
  const { messages, clearMessages } = useMessagesFeed();
  const { pipelineLoading, droppedChunks, loadError, resetPipelineErrors } = useLocalPipeline();
  const [voiceActive, setVoiceActive] = useState(false);
  const prevStateRef = useRef(listeningState);

  const isActive = listeningState === "listening";
  const timer = useListeningTimer(isActive);

  useEffect(() => {
    const prev = prevStateRef.current;
    if (listeningState === "listening" && prev !== "listening") {
      clearMessages();
      resetPipelineErrors();
    }
    prevStateRef.current = listeningState;
  }, [listeningState, clearMessages, resetPipelineErrors]);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;
    listen<"speech-start" | "speech-end">(VAD_STATUS_EVENT, (event) => {
      if (!cancelled) setVoiceActive(event.payload === "speech-start");
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

  const handleCopy = useCallback(() => {
    const textToCopy = messages.map((m) => m.text).join("\n");
    if (!textToCopy.trim()) return;
    void navigator.clipboard.writeText(textToCopy);
  }, [messages]);

  return (
    <div className="relative flex h-full flex-col overflow-hidden rounded-xl bg-transparent">
      {loadError && (
        <div className="flex shrink-0 items-center gap-2 border-b border-red-500/20 bg-red-500/5 px-4 py-1.5">
          <span className="text-[11px] text-red-400">{loadError}</span>
        </div>
      )}
      {droppedChunks > 0 && pipelineLoading && isActive && (
        <div className="flex shrink-0 items-center gap-2 border-b border-yellow-500/20 bg-yellow-500/5 px-4 py-1.5">
          <span className="text-[11px] text-yellow-400">
            Some audio was not captured while the model was loading
          </span>
        </div>
      )}

      <header className="flex shrink-0 select-none items-center justify-between border-b-[0.5px] border-white/8 px-4 pb-1.5 pt-2.5">
        <div className="flex items-center gap-1.5">
          {isActive && (
            <span
              className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${
                voiceActive
                  ? "bg-[#30d158] shadow-[0_0_6px_rgba(48,209,88,0.6)] animate-pulse-dot"
                  : "bg-white/30"
              }`}
            />
          )}
          <span className="text-[11px] font-medium text-[--text-color]">Transcript</span>
          {isActive && (
            <span className="text-[10px] tabular-nums text-[--text-dimmed]">{timer}</span>
          )}
        </div>

        <button
          onClick={handleCopy}
          disabled={messages.length === 0}
          title="Copy to clipboard"
          className={`flex items-center justify-center rounded-md border-none bg-transparent p-1.5 transition-colors duration-150 ${
            messages.length > 0
              ? "cursor-pointer text-[--text-muted] hover:bg-[--hover-bg] hover:text-[--text-color]"
              : "cursor-default text-[--text-dimmed] opacity-50"
          }`}
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
            <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
          </svg>
        </button>
      </header>

      <div className="relative flex-1 min-h-0 overflow-hidden">
        {!isActive && messages.length === 0 ? (
          <div className="flex h-full items-center justify-center px-6">
            <p className="text-center text-[12px] leading-relaxed tracking-[-0.01em] text-[--text-dimmed]">
              Press Listen to start capturing audio
            </p>
          </div>
        ) : (
          <TranscriptView messages={messages} />
        )}
      </div>
    </div>
  );
}
