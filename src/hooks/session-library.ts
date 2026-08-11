import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useReplyInFlight, lastUnansweredUserId } from "./reply-in-flight";

export interface ChatMsg {
  id: string;
  role: string;
  content: string;
  status: string;
  parent_message_id?: string | null;
  error_code?: string | null;
  error_message?: string | null;
  created_at: string;
}

export type TranscriptSource = "you" | "others";

export interface TranscriptSegment {
  source: TranscriptSource;
  utterance: string;
  speaker_id?: string | null;
  speaker_label?: string | null;
}

export interface SessionSpeaker {
  speaker_id: string;
  speaker_label?: string | null;
}

export interface SessionRecord {
  session_id: string;
  relay_session_id: string;
  started_at: string;
  ended_at: string;
  title?: string | null;
  segments: TranscriptSegment[];
  notes_markdown?: string | null;
  notes_status?: string | null;
  notes_error?: string | null;
  notes_root_message_id?: string | null;
  chat: ChatMsg[];
}

export interface SessionSummary {
  session_id: string;
  title?: string | null;
  ended_at: string;
  has_notes: boolean;
  notes_status?: string | null;
}

interface RawReplyMessage {
  id: string;
  session_id: string;
  role: string;
  content: string;
  status: string;
  parent_message_id?: string | null;
  error_code?: string | null;
  error_message?: string | null;
  created_at: string;
}

interface NotesPendingPayload {
  session_id: string;
  user_message_id: string;
}

interface NotesErrorPayload {
  session_id: string;
  message: string;
}

interface ChatErroredPayload {
  task_id: string;
  session_id: string;
  user_message_id?: string | null;
  error_code?: string | null;
  error_message?: string | null;
}

export function useSessions() {
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const list = await invoke<SessionSummary[]>("list_sessions");
      setSessions(list);
    } catch {
      setSessions([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const remove = useCallback(async (sessionId: string) => {
    await invoke("delete_session", { sessionId });
    setSessions((prev) => prev.filter((s) => s.session_id !== sessionId));
  }, []);

  useEffect(() => {
    const unlistens = [
      listen("session-finalized", () => void refresh()),
      listen("session-updated", () => void refresh()),
    ];
    return () => {
      unlistens.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, [refresh]);

  return { sessions, loading, refresh, remove };
}

function applyReply(record: SessionRecord, reply: RawReplyMessage): SessionRecord {
  const isNotesReply =
    record.notes_root_message_id != null &&
    record.notes_root_message_id === reply.parent_message_id;

  if (isNotesReply) {
    if (reply.status === "completed") {
      return {
        ...record,
        notes_markdown: reply.content,
        notes_status: "completed",
        notes_error: null,
      };
    }
    if (reply.status === "errored") {
      return {
        ...record,
        notes_status: "errored",
        notes_error: reply.error_message ?? reply.error_code ?? null,
      };
    }
    return { ...record, notes_status: reply.status };
  }

  if (record.chat.some((m) => m.id === reply.id)) {
    return record;
  }

  const assistant: ChatMsg = {
    id: reply.id,
    role: reply.role,
    content: reply.content,
    status: reply.status,
    parent_message_id: reply.parent_message_id ?? null,
    error_code: reply.error_code ?? null,
    error_message: reply.error_message ?? null,
    created_at: reply.created_at,
  };
  return { ...record, chat: [...record.chat, assistant] };
}

export function useSessionRecord(sessionId: string | null) {
  const [record, setRecord] = useState<SessionRecord | null>(null);
  const [loading, setLoading] = useState(false);
  const [sending, setSending] = useState(false);
  const {
    inFlight,
    begin,
    resolveReply,
    resolveError,
  } = useReplyInFlight(sessionId);
  const recordRef = useRef<SessionRecord | null>(null);

  recordRef.current = record;

  const rehydratePending = useCallback(
    async (id: string, loaded: SessionRecord | null) => {
      if (!loaded) return;
      const target = lastUnansweredUserId(loaded.chat);
      if (!target) return;
      const pending = await invoke<boolean>("local_turn_pending", { sessionId: id });
      if (pending) begin(target);
    },
    [begin],
  );

  const reload = useCallback(async () => {
    if (!sessionId) {
      setRecord(null);
      return;
    }
    setLoading(true);
    try {
      const loaded = await invoke<SessionRecord | null>("get_session", { sessionId });
      setRecord(loaded);
      await rehydratePending(sessionId, loaded);
    } catch {
      setRecord(null);
    } finally {
      setLoading(false);
    }
  }, [sessionId, rehydratePending]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    if (!sessionId) return;
    const matches = (sid: string) => sid === sessionId;
    const unlistens: Array<Promise<() => void>> = [
      listen<NotesPendingPayload>("notes-pending", (e) => {
        if (!matches(e.payload.session_id)) return;
        setRecord((prev) =>
          prev ? { ...prev, notes_status: "pending", notes_error: null } : prev,
        );
      }),
      listen<NotesErrorPayload>("notes-error", (e) => {
        if (!matches(e.payload.session_id)) return;
        setRecord((prev) =>
          prev ? { ...prev, notes_status: "errored", notes_error: e.payload.message } : prev,
        );
      }),
      listen<RawReplyMessage>("chat-message-created", (e) => {
        if (!matches(e.payload.session_id)) return;
        resolveReply(e.payload);
        setRecord((prev) => (prev ? applyReply(prev, e.payload) : prev));
      }),
      listen<ChatErroredPayload>("chat-message-errored", (e) => {
        if (!matches(e.payload.session_id)) return;
        resolveError(e.payload.user_message_id);
        setRecord((prev) => {
          if (!prev) return prev;
          if (
            prev.notes_root_message_id != null &&
            prev.notes_root_message_id === e.payload.user_message_id &&
            prev.notes_markdown == null
          ) {
            return {
              ...prev,
              notes_status: "errored",
              notes_error:
                e.payload.error_message ?? e.payload.error_code ?? "Assistant error",
            };
          }
          return prev;
        });
      }),
    ];
    return () => {
      unlistens.forEach((p) => p.then((fn) => fn()).catch(() => {}));
    };
  }, [sessionId]);

  const sendMessage = useCallback(
    async (text: string) => {
      const current = recordRef.current;
      if (!sessionId || !current || sending) return;
      const trimmed = text.trim();
      if (!trimmed) return;

      const lastAssistant = [...current.chat].reverse().find((m) => m.role === "assistant");
      const parent = lastAssistant?.id ?? current.notes_root_message_id ?? null;

      setSending(true);
      try {
        const userMsg = await invoke<ChatMsg>("send_session_chat_message", {
          sessionId,
          text: trimmed,
          parentMessageId: parent,
        });
        begin(userMsg.id);
        setRecord((prev) =>
          prev && !prev.chat.some((m) => m.id === userMsg.id)
            ? { ...prev, chat: [...prev.chat, userMsg] }
            : prev,
        );
      } finally {
        setSending(false);
      }
    },
    [sessionId, sending],
  );

  const updateTitle = useCallback(
    async (title: string | null) => {
      if (!sessionId) return;
      const trimmed = title?.trim() ?? null;
      const next = trimmed && trimmed.length > 0 ? trimmed : null;
      await invoke("update_session_title", { sessionId, title: next });
      setRecord((prev) => (prev ? { ...prev, title: next } : prev));
    },
    [sessionId],
  );

  const awaitingReply = sending || inFlight;

  return { record, loading, sending, awaitingReply, sendMessage, updateTitle, reload };
}

export function useSessionSpeakers(sessionId: string | null) {
  const [speakers, setSpeakers] = useState<SessionSpeaker[]>([]);

  const refresh = useCallback(async () => {
    if (!sessionId) {
      setSpeakers([]);
      return;
    }
    try {
      const list = await invoke<SessionSpeaker[]>("list_session_speakers", { sessionId });
      setSpeakers(list);
    } catch {
      setSpeakers([]);
    }
  }, [sessionId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const rename = useCallback(
    async (speakerId: string, label: string | null) => {
      const trimmed = label?.trim() ?? null;
      const next = trimmed && trimmed.length > 0 ? trimmed : null;
      await invoke("rename_speaker", { speakerId, label: next });
      await refresh();
    },
    [refresh],
  );

  return { speakers, refresh, rename };
}
