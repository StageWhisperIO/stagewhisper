import { useCallback, useEffect, useMemo, useState } from "react";
import { useChat } from "@ai-sdk/react";
import { invoke } from "@tauri-apps/api/core";
import {
  chatMessageToUI,
  TauriChatTransport,
  type ChatMsg,
  type ChatUIMessage,
} from "@/lib/tauri-chat-transport";
import { userFacingError } from "@/lib/utils";

interface ChatActivity {
  label: string;
}

export function useTauriChat({ sessionId }: { sessionId: string }) {
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [activity, setActivity] = useState<ChatActivity | null>(null);
  const transport = useMemo(() => new TauriChatTransport(sessionId), [sessionId]);
  const chat = useChat<ChatUIMessage>({
    id: sessionId,
    transport,
    resume: true,
    onData: (dataPart) => {
      if (dataPart.type === "data-activity") setActivity(dataPart.data);
    },
  });

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setLoadError(null);
    invoke<{ chat: ChatMsg[] } | null>("get_session", { sessionId })
      .then((record) => {
        if (cancelled) return;
        const messages = (record?.chat ?? []).map(chatMessageToUI);
        chat.setMessages((current) => {
          const currentIds = new Set(current.map((m) => m.id));
          const missing = messages.filter((m) => !currentIds.has(m.id));
          return missing.length > 0 ? [...missing, ...current] : current;
        });
      })
      .catch((err) => {
        if (!cancelled) setLoadError(userFacingError(err, "Couldn't load this chat. Try again."));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  useEffect(() => {
    if (chat.status === "ready" || chat.status === "error") setActivity(null);
  }, [chat.status]);

  const send = useCallback(
    async (text: string) => {
      chat.clearError();
      setActivity(null);
      await chat.sendMessage({ text });
    },
    [chat],
  );

  const clear = useCallback(() => chat.setMessages([]), [chat]);
  const chatError = chat.error;
  const sendError = useMemo(
    () => (chatError ? userFacingError(chatError, "Couldn't send that message. Try again.") : null),
    [chatError],
  );

  return {
    messages: chat.messages,
    loading,
    sending: chat.status === "submitted" || chat.status === "streaming",
    activity,
    error: loadError ?? sendError,
    send,
    clear,
  };
}
