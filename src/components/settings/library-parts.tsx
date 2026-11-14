import { useEffect, useState } from "react";
import type { SessionSpeaker } from "@/hooks/session-library";
import { useTranslation } from "@/i18n";

export function TrashIcon() {
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

export function TypingDots() {
  return (
    <span className="inline-flex items-center gap-1">
      <span className="h-1.5 w-1.5 animate-typing-dot rounded-full bg-current [animation-delay:0ms]" />
      <span className="h-1.5 w-1.5 animate-typing-dot rounded-full bg-current [animation-delay:150ms]" />
      <span className="h-1.5 w-1.5 animate-typing-dot rounded-full bg-current [animation-delay:300ms]" />
    </span>
  );
}

export function SessionTitle({
  title,
  onSave,
}: {
  title: string;
  onSave: (next: string | null) => Promise<void>;
}) {
  const { t } = useTranslation("library");
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
      title={t("detail.rename")}
      className="flex-1 truncate rounded-md px-2 py-1 text-left text-sm font-medium text-heading transition hover:bg-sidebar-active"
    >
      {title}
    </button>
  );
}

function speakerName(speaker: SessionSpeaker): string {
  return speaker.speaker_label?.trim() || speaker.speaker_id;
}

export function SpeakerRename({
  speakers,
  onRename,
}: {
  speakers: SessionSpeaker[];
  onRename: (speakerId: string, label: string | null) => Promise<void>;
}) {
  const { t } = useTranslation("library");
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
            title={t("detail.renameSpeaker")}
            className="rounded-none border border-rule px-3 py-1 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
          >
            {label}
          </button>
        );
      })}
    </div>
  );
}
