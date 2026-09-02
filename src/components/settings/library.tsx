import { useEffect, useMemo, useRef, useState } from "react";
import {
  useSessionRecord,
  useSessions,
  useSessionSpeakers,
  type SessionSpeaker,
  type SessionSummary,
} from "@/hooks/session-library";
import { useTauriChat } from "@/hooks/use-tauri-chat";
import { chatMessageToUI, uiMessageText, type ChatUIMessage } from "@/lib/tauri-chat-transport";
import { useCapabilities } from "@/hooks/edition";
import { Markdown } from "@/lib/markdown";
import { cn } from "@/lib/utils";
import { EmptyState } from "./primitives";

function formatDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function statusLabel(s: SessionSummary): string {
  if (s.has_notes) return "Notes ready";
  switch (s.notes_status) {
    case "pending":
      return "Summarizing…";
    case "errored":
      return "Notes failed";
    case "cancelled":
      return "No notes";
    default:
      return "Transcript only";
  }
}

export function LibrarySection({
  initialSessionId = null,
}: {
  initialSessionId?: string | null;
}) {
  const [sessionId, setSessionId] = useState<string | null>(initialSessionId);

  if (sessionId) {
    return <SessionDetail sessionId={sessionId} onBack={() => setSessionId(null)} />;
  }
  return <LibraryList onOpen={setSessionId} />;
}

function TrashIcon() {
  return (
    <svg
      width="15"
      height="15"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M3 6h18" />
      <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
      <line x1="10" y1="11" x2="10" y2="17" />
      <line x1="14" y1="11" x2="14" y2="17" />
    </svg>
  );
}

function LibraryList({ onOpen }: { onOpen: (id: string) => void }) {
  const { sessions, loading, refresh, remove } = useSessions();
  const [armedId, setArmedId] = useState<string | null>(null);
  const armTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const clearArm = () => {
    if (armTimer.current) {
      clearTimeout(armTimer.current);
      armTimer.current = null;
    }
    setArmedId(null);
  };

  useEffect(() => () => {
    if (armTimer.current) clearTimeout(armTimer.current);
  }, []);

  const handleDelete = async (id: string) => {
    if (armedId === id) {
      clearArm();
      try {
        await remove(id);
      } catch {
        /* keep the row if deletion fails */
      }
      return;
    }
    setArmedId(id);
    if (armTimer.current) clearTimeout(armTimer.current);
    armTimer.current = setTimeout(() => setArmedId(null), 3500);
  };

  return (
    <div className="flex h-full flex-col overflow-y-auto px-8 py-6">
      <header className="mb-5 flex items-start justify-between">
        <div>
          <h1 className="text-lg font-semibold text-heading">Library</h1>
          <p className="mt-1 text-sm text-muted">
            Your past sessions. Transcripts and notes are stored encrypted on this device.
          </p>
        </div>
        <button
          onClick={() => void refresh()}
          className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
        >
          Refresh
        </button>
      </header>

      {loading && <p className="text-sm text-muted">Loading…</p>}

      {!loading && sessions.length === 0 && (
        <div className="rounded-xl border border-dashed border-rule-strong p-8 text-center text-sm text-muted">
          No sessions yet. Finish a call to capture a transcript and summary.
        </div>
      )}

      <ul className="space-y-1">
        {sessions.map((s) => {
          const armed = armedId === s.session_id;
          return (
            <li
              key={s.session_id}
              className="group flex items-center gap-1 rounded-lg pr-2 hover:bg-sidebar-active"
            >
              <button
                onClick={() => onOpen(s.session_id)}
                className="min-w-0 flex-1 rounded-lg px-3 py-2.5 text-left"
              >
                <div className="truncate text-sm font-medium text-heading">
                  {s.title?.trim() || formatDate(s.ended_at)}
                </div>
                <div className="mt-0.5 flex items-center justify-between gap-2 text-xs">
                  <span className="text-muted">{formatDate(s.ended_at)}</span>
                  {armed ? (
                    <span className="text-danger">Click trash again to permanently delete</span>
                  ) : (
                    <span className="text-muted">{statusLabel(s)}</span>
                  )}
                </div>
              </button>
              <button
                onClick={() => void handleDelete(s.session_id)}
                title={armed ? "Click again to permanently delete" : "Delete session"}
                aria-label={armed ? "Confirm delete session" : "Delete session"}
                className={cn(
                  "shrink-0 rounded-md p-2 transition",
                  armed
                    ? "bg-danger-bg text-danger"
                    : "text-muted opacity-0 hover:text-danger group-hover:opacity-100",
                )}
              >
                <TrashIcon />
              </button>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

type DetailTab = "summary" | "transcript";

function TypingDots() {
  return (
    <span className="inline-flex items-center gap-1">
      <span className="h-1.5 w-1.5 animate-typing-dot rounded-full bg-current [animation-delay:0ms]" />
      <span className="h-1.5 w-1.5 animate-typing-dot rounded-full bg-current [animation-delay:150ms]" />
      <span className="h-1.5 w-1.5 animate-typing-dot rounded-full bg-current [animation-delay:300ms]" />
    </span>
  );
}

function SessionTitle({
  title,
  onSave,
}: {
  title: string;
  onSave: (next: string | null) => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(title);

  useEffect(() => {
    if (!editing) setDraft(title);
  }, [title, editing]);

  const commit = async () => {
    setEditing(false);
    if (draft.trim() === title.trim()) return;
    try {
      await onSave(draft);
    } catch {
      setDraft(title);
    }
  };

  if (editing) {
    return (
      <input
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            void commit();
          } else if (e.key === "Escape") {
            setDraft(title);
            setEditing(false);
          }
        }}
        className="flex-1 rounded-md border border-rule-strong bg-page px-2 py-1 text-sm font-medium text-heading outline-none focus:border-accent"
      />
    );
  }

  return (
    <button
      onClick={() => setEditing(true)}
      title="Rename session"
      className="flex-1 truncate rounded-md px-2 py-1 text-left text-sm font-medium text-heading transition hover:bg-sidebar-active"
    >
      {title}
    </button>
  );
}

function speakerName(speaker: SessionSpeaker): string {
  return speaker.speaker_label?.trim() || speaker.speaker_id;
}

function SpeakerRename({
  speakers,
  onRename,
}: {
  speakers: SessionSpeaker[];
  onRename: (speakerId: string, label: string | null) => Promise<void>;
}) {
  const [editing, setEditing] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  if (speakers.length === 0) return null;

  const commit = async (speakerId: string, fallback: string) => {
    setEditing(null);
    if (draft.trim() === fallback.trim()) return;
    try {
      await onRename(speakerId, draft);
    } catch {
      setDraft(fallback);
    }
  };

  return (
    <div className="mb-3 flex flex-wrap gap-2 border-b border-rule pb-3">
      {speakers.map((speaker) => {
        const label = speakerName(speaker);
        if (editing === speaker.speaker_id) {
          return (
            <input
              key={speaker.speaker_id}
              autoFocus
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onBlur={() => void commit(speaker.speaker_id, label)}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  e.preventDefault();
                  void commit(speaker.speaker_id, label);
                } else if (e.key === "Escape") {
                  setEditing(null);
                }
              }}
              className="rounded-md border border-rule-strong bg-page px-2 py-1 text-xs text-body outline-none focus:border-accent"
            />
          );
        }
        return (
          <button
            key={speaker.speaker_id}
            onClick={() => {
              setDraft(speaker.speaker_label?.trim() || "");
              setEditing(speaker.speaker_id);
            }}
            title="Rename speaker"
            className="rounded-none border border-rule px-3 py-1 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
          >
            {label}
          </button>
        );
      })}
    </div>
  );
}

function SessionDetail({ sessionId, onBack }: { sessionId: string; onBack: () => void }) {
  const { record, loading, updateTitle, reload } = useSessionRecord(sessionId);
  const chat = useTauriChat({ sessionId });
  const capabilities = useCapabilities();
  const diarization = capabilities.diarization;
  const { speakers, rename: renameSpeaker } = useSessionSpeakers(
    diarization ? sessionId : null,
  );
  const handleRenameSpeaker = async (speakerId: string, label: string | null) => {
    await renameSpeaker(speakerId, label);
    void reload().catch(() => {});
  };
  const [draft, setDraft] = useState("");
  const [tab, setTab] = useState<DetailTab>("summary");
  const threadRef = useRef<HTMLDivElement>(null);

  const summaryArrived = Boolean(
    record?.notes_root_message_id &&
      record.chat.some(
        (m) => m.role === "assistant" && m.parent_message_id === record.notes_root_message_id,
      ),
  );
  const notesStatus = record?.notes_status ?? null;
  const summaryPending = notesStatus === "pending" && !summaryArrived;

  const notesReply = useMemo(() => {
    if (!record?.notes_root_message_id) return null;
    return (
      record.chat.find(
        (m) => m.role === "assistant" && m.parent_message_id === record.notes_root_message_id,
      ) ?? null
    );
  }, [record]);

  const threadMessages = useMemo<ChatUIMessage[]>(() => {
    if (notesReply && !chat.messages.some((m) => m.id === notesReply.id)) {
      return [chatMessageToUI(notesReply), ...chat.messages];
    }
    return chat.messages;
  }, [chat.messages, notesReply]);

  useEffect(() => {
    if (threadRef.current) {
      threadRef.current.scrollTop = threadRef.current.scrollHeight;
    }
  }, [threadMessages.length, chat.sending, chat.activity, summaryPending]);

  const notesReady = Boolean(record?.notes_markdown);

  const onSubmit = async () => {
    const text = draft.trim();
    if (!notesReady || chat.sending || chat.loading || !text) return;
    setDraft("");
    await chat.send(text);
  };

  const tabButton = (id: DetailTab, label: string) => (
    <button
      onClick={() => setTab(id)}
      className={cn(
        "rounded-lg px-2.5 py-1 text-sm font-medium transition",
        tab === id
          ? "bg-sidebar-active text-heading"
          : "text-muted hover:bg-sidebar-active hover:text-heading",
      )}
    >
      {label}
    </button>
  );

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex items-center gap-2 border-b border-rule px-6 py-3">
        <button
          onClick={onBack}
          className="rounded-lg px-2 py-1 text-sm font-medium text-muted transition hover:bg-sidebar-active hover:text-heading"
        >
          ← Library
        </button>
        {record ? (
          <SessionTitle
            title={record.title?.trim() || "Session"}
            onSave={(next) => updateTitle(next)}
          />
        ) : (
          <div className="flex-1 truncate text-sm font-medium text-heading">Session</div>
        )}
        {tabButton("summary", "Summary")}
        {tabButton("transcript", "Transcript")}
      </div>

      {loading && <p className="p-6 text-sm text-muted">Loading…</p>}

      {!loading && record && tab === "summary" && chat.loading && (
        <p className="p-6 text-sm text-muted">Loading…</p>
      )}

      {!loading && record && tab === "summary" && !chat.loading && (
        <>
          <div ref={threadRef} className="flex-1 overflow-y-auto px-6 py-4">
            <div className="space-y-2">
              {threadMessages
                .filter(
                  (m) =>
                    m.role !== "assistant" ||
                    m.metadata?.status === "errored" ||
                    (notesReply != null && m.id === notesReply.id && Boolean(record.notes_markdown)) ||
                    uiMessageText(m).trim().length > 0,
                )
                .map((m) => {
                const text = uiMessageText(m);
                const isNotesReply = notesReply != null && m.id === notesReply.id;
                const errored = m.metadata?.status === "errored";
                return (
                  <div
                    key={m.id}
                    className={cn(
                      "max-w-[85%] rounded-xl px-3 py-2 text-sm",
                      m.role === "user"
                        ? "ml-auto bg-accent text-accent-fg"
                        : "mr-auto bg-sidebar-active text-body",
                    )}
                  >
                    {errored ? (
                      <div className="flex items-center gap-2">
                        <span className="text-danger">
                          {m.role === "user" ? "This message failed to send." : "This reply failed."}
                        </span>
                        {m.role === "user" && (
                          <button
                            onClick={() => void chat.send(text)}
                            className="shrink-0 rounded border border-rule-strong px-2 py-0.5 text-[10px] font-medium text-body transition hover:border-accent hover:text-accent"
                          >
                            Retry
                          </button>
                        )}
                      </div>
                    ) : m.role === "assistant" ? (
                      <Markdown
                        source={isNotesReply && record.notes_markdown ? record.notes_markdown : text}
                      />
                    ) : (
                      <span className="whitespace-pre-wrap">{text}</span>
                    )}
                  </div>
                );
              })}
              {summaryPending && (
                <div className="mr-auto flex max-w-[85%] items-center gap-2 rounded-xl bg-sidebar-active px-3 py-2.5 text-muted">
                  <span className="text-sm">Your assistant is writing the summary</span>
                  <TypingDots />
                </div>
              )}
              {notesStatus === "errored" && !summaryArrived && (
                <div className="mr-auto max-w-[85%] rounded-xl bg-sidebar-active px-3 py-2.5">
                  <p className="text-sm text-danger">Couldn't generate the summary.</p>
                </div>
              )}
              {chat.sending && (
                <div className="mr-auto flex max-w-[85%] items-center gap-2 rounded-xl bg-sidebar-active px-3 py-2.5 text-muted">
                  <TypingDots />
                  {chat.activity && (
                    <span className="text-xs">{chat.activity.label?.trim() || "Thinking…"}</span>
                  )}
                </div>
              )}
            </div>
          </div>

          <form
            className="flex items-end gap-2 border-t border-rule px-6 py-3"
            onSubmit={(e) => {
              e.preventDefault();
              void onSubmit();
            }}
          >
            <div className="flex flex-1 flex-col gap-1">
              <textarea
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    void onSubmit();
                  }
                }}
                rows={1}
                disabled={!notesReady}
                placeholder={
                  notesReady
                    ? "Refine the action items…"
                    : notesStatus === "errored"
                      ? "Chat unavailable, the summary failed."
                      : "Chat unlocks once the summary is ready…"
                }
                className="resize-none rounded-lg border border-rule-strong bg-page px-3 py-2 text-sm text-body outline-none focus:border-accent disabled:cursor-not-allowed disabled:opacity-50"
              />
              {chat.error && <p className="text-xs text-danger">{chat.error}</p>}
            </div>
            <button
              type="submit"
              disabled={!notesReady || chat.sending || chat.loading || !draft.trim()}
              className="rounded-lg bg-accent px-4 py-2 text-sm font-medium text-accent-fg transition hover:bg-accent-hover disabled:opacity-50"
            >
              Send
            </button>
          </form>
        </>
      )}

      {!loading && record && tab === "transcript" && (
        <div className="flex-1 overflow-y-auto px-6 py-4">
          {diarization && (
            <SpeakerRename speakers={speakers} onRename={handleRenameSpeaker} />
          )}
          {record.segments.length > 0 ? (
            <div className="space-y-1.5">
              {record.segments.map((segment, index) => {
                const isYou = segment.source === "you";
                const prev = record.segments[index - 1];
                const isFirst =
                  index === 0 ||
                  prev.source !== segment.source ||
                  (diarization && prev.speaker_id !== segment.speaker_id);
                const othersLabel =
                  diarization && segment.speaker_id
                    ? segment.speaker_label?.trim() || segment.speaker_id
                    : "Others";
                return (
                  <div key={index} className={cn("flex", isYou ? "justify-end" : "justify-start")}>
                    <div
                      className={cn(
                        "max-w-[80%] rounded-2xl px-3.5 py-2 text-sm leading-relaxed",
                        isYou
                          ? "rounded-br-md bg-accent text-accent-fg"
                          : "rounded-bl-md border border-rule bg-sidebar-active text-body",
                      )}
                    >
                      {isFirst && (
                        <span
                          className={cn(
                            "mb-0.5 block text-[10px] font-medium",
                            isYou ? "text-right text-accent-fg" : "text-muted",
                          )}
                        >
                          {isYou ? "You" : othersLabel}
                        </span>
                      )}
                      <p className="whitespace-pre-wrap">{segment.utterance}</p>
                    </div>
                  </div>
                );
              })}
            </div>
          ) : (
            <EmptyState message="No transcript." />
          )}
        </div>
      )}

      {!loading && !record && <EmptyState message="Session not found." />}
    </div>
  );
}
