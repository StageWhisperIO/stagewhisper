import { useCallback, useEffect, useRef, useState } from "react";
import { UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

const TRANSCRIPT_EVENT = "transcript-updated";
const SIGNAL_EVENT = "signal-created";

const MAX_LIVE_MESSAGES = 2000;
const MAX_LIVE_SIGNALS = 500;

export type MessageKind = "input" | "output";
export type TranscriptSource = "you" | "others";

export interface Message {
  id: number;
  kind: MessageKind;
  text: string;
  timestamp: number;
  source?: TranscriptSource;
}

export interface Signal {
  id: number;
  severity: "red" | "orange" | "green";
  message: string;
  timestamp: number;
}

interface TranscriptEventPayload {
  kind: string;
  text: string;
  finished: boolean;
  source?: TranscriptSource;
}

interface SignalEventPayload {
  severity: string;
  message: string;
}

let nextMessageId = 0;
let nextSignalId = 0;

function createMessage(kind: MessageKind, text: string, source: TranscriptSource): Message {
  return {
    id: nextMessageId++,
    kind,
    text,
    timestamp: Date.now(),
    source,
  };
}

function normalizeSeverity(raw: string): Signal["severity"] {
  const lower = raw.toLowerCase();
  if (lower === "red" || lower === "orange" || lower === "green") return lower;
  return "green";
}

const NO_SIGNAL_PATTERNS = new Set(["no_signal", "no signal", "nosignal"]);

function isNoSignal(text: string): boolean {
  return NO_SIGNAL_PATTERNS.has(text.trim().replace(/[_.]/g, "").toLowerCase());
}

export function useMessagesFeed() {
  const [messages, setMessages] = useState<Message[]>([]);
  const pendingKindRef = useRef<MessageKind | null>(null);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    const setup = async () => {
      try {
        const appWindow = getCurrentWebviewWindow();
        unlisten = await appWindow.listen<TranscriptEventPayload>(TRANSCRIPT_EVENT, (event) => {
          if (cancelled) return;

          const { kind, text, finished, source } = event.payload;
          if (kind !== "input" && kind !== "output") return;
          const messageKind: MessageKind = kind;
          const messageSource: TranscriptSource = source ?? "others";

          setMessages((prev) => {
            const last = prev.length > 0 ? prev[prev.length - 1] : null;
            const isContinuation =
              last !== null &&
              last.kind === messageKind &&
              last.source === messageSource &&
              pendingKindRef.current === messageKind;

            if (finished) {
              pendingKindRef.current = null;
              if (text && isContinuation) {
                const updated = [...prev];
                const separator = last!.text.length > 0 && !last!.text.endsWith(" ") ? " " : "";
                updated[updated.length - 1] = {
                  ...last!,
                  text: last!.text + separator + text,
                };
                return updated;
              }
              return prev;
            }

            if (!text) return prev;

            if (isContinuation) {
              const updated = [...prev];
              const separator = last!.text.length > 0 && !last!.text.endsWith(" ") ? " " : "";
              updated[updated.length - 1] = {
                ...last!,
                text: last!.text + separator + text,
              };
              return updated;
            }

            pendingKindRef.current = messageKind;
            const next = [...prev, createMessage(messageKind, text, messageSource)];
            if (next.length > MAX_LIVE_MESSAGES) {
              return next.slice(next.length - MAX_LIVE_MESSAGES);
            }
            return next;
          });
        });
      } catch (err) {
        console.error("[useMessagesFeed] listen() failed:", err);
      }
    };

    setup();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const clearMessages = useCallback(() => {
    setMessages([]);
    pendingKindRef.current = null;
  }, []);

  const inputMessages = messages.filter((m) => m.kind === "input");

  return { messages, inputMessages, clearMessages };
}

export function useSignalsFeed() {
  const [signals, setSignals] = useState<Signal[]>([]);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    const setup = async () => {
      try {
        const appWindow = getCurrentWebviewWindow();
        unlisten = await appWindow.listen<SignalEventPayload>(SIGNAL_EVENT, (event) => {
          if (cancelled) return;

          const { severity, message } = event.payload;
          if (!message.trim() || isNoSignal(message)) return;

          setSignals((prev) => {
            const next = [
              ...prev,
              {
                id: nextSignalId++,
                severity: normalizeSeverity(severity),
                message: message.trim(),
                timestamp: Date.now(),
              },
            ];
            if (next.length > MAX_LIVE_SIGNALS) {
              return next.slice(next.length - MAX_LIVE_SIGNALS);
            }
            return next;
          });
        });
      } catch (err) {
        console.error("[useSignalsFeed] listen() failed:", err);
      }
    };

    setup();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  const clearSignals = useCallback(() => {
    setSignals([]);
  }, []);

  return { signals, clearSignals };
}
