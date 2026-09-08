import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { ChatMsg } from "@/lib/tauri-chat-transport";
import { userFacingError } from "@/lib/utils";

export type { ChatMsg };

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
  const recordRef = useRef<SessionRecord | null>(null);

  recordRef.current = record;

  const reload = useCallback(async () => {
    if (!sessionId) {
      setRecord(null);
      return;
    }
    setLoading(true);
    try {
      const loaded = await invoke<SessionRecord | null>("get_session", { sessionId });
      setRecord(loaded);
    } catch {
      setRecord(null);
    } finally {
      setLoading(false);
    }
  }, [sessionId]);

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
          prev
            ? {
                ...prev,
                notes_status: "pending",
                notes_error: null,
                notes_root_message_id: prev.notes_root_message_id ?? e.payload.user_message_id,
              }
            : prev,
        );
      }),
      listen<NotesErrorPayload>("notes-error", (e) => {
        if (!matches(e.payload.session_id)) return;
        setRecord((prev) =>
          prev
            ? {
                ...prev,
                notes_status: "errored",
                notes_error: userFacingError(e.payload, "Couldn't generate the summary."),
              }
            : prev,
        );
      }),
      listen<RawReplyMessage>("chat-message-created", (e) => {
        if (!matches(e.payload.session_id)) return;
        setRecord((prev) => (prev ? applyReply(prev, e.payload) : prev));
        const reply = e.payload;
        if (
          reply.role === "assistant" &&
          reply.status === "completed" &&
          reply.parent_message_id != null &&
          reply.parent_message_id === recordRef.current?.notes_root_message_id
        ) {
          void reload();
        }
      }),
      listen<ChatErroredPayload>("chat-message-errored", (e) => {
        if (!matches(e.payload.session_id)) return;
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
              notes_error: userFacingError(e.payload, "Couldn't generate the summary."),
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

  return { record, loading, updateTitle, reload };
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
