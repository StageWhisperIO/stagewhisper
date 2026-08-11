import { useCallback, useEffect, useRef, useState } from "react";

export interface AgentActivity {
  phase: "reasoning" | "tool_call";
  label: string | null;
}

export interface ReplyLike {
  role: string;
  status: string;
  parent_message_id?: string | null;
}

const TERMINAL_STATUSES = ["completed", "errored", "cancelled"];

export interface StoredChatEntry {
  id: string;
  role: string;
  status: string;
  parent_message_id?: string | null;
}

export function lastUnansweredUserId(entries: StoredChatEntry[]): string | null {
  const answered = new Set(
    entries
      .filter((m) => m.role === "assistant" && m.parent_message_id != null)
      .map((m) => m.parent_message_id as string),
  );
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    const entry = entries[i];
    if (entry.role !== "user") continue;
    if (entry.status === "errored" || entry.status === "cancelled") return null;
    return answered.has(entry.id) ? null : entry.id;
  }
  return null;
}

export function useReplyInFlight(sessionId: string | null) {
  const [inFlight, setInFlight] = useState(false);
  const [agentActivity, setAgentActivity] = useState<AgentActivity | null>(null);
  const targetRef = useRef<string | null>(null);
  const settledEarlyRef = useRef(new Set<string>());

  const clear = useCallback(() => {
    targetRef.current = null;
    setInFlight(false);
    setAgentActivity(null);
  }, []);

  useEffect(() => {
    settledEarlyRef.current.clear();
    clear();
  }, [sessionId, clear]);

  const begin = useCallback((userMessageId: string) => {
    if (settledEarlyRef.current.delete(userMessageId)) return;
    targetRef.current = userMessageId;
    setInFlight(true);
    setAgentActivity(null);
  }, []);

  const settle = useCallback(
    (userMessageId: string) => {
      if (targetRef.current === userMessageId) {
        clear();
        return;
      }
      if (targetRef.current === null) {
        settledEarlyRef.current.add(userMessageId);
      }
    },
    [clear],
  );

  const resolveReply = useCallback(
    (reply: ReplyLike) => {
      if (reply.role !== "assistant") return;
      if (reply.parent_message_id == null) return;
      if (!TERMINAL_STATUSES.includes(reply.status)) return;
      settle(reply.parent_message_id);
    },
    [settle],
  );

  const resolveError = useCallback(
    (userMessageId?: string | null) => {
      if (!userMessageId) return;
      settle(userMessageId);
    },
    [settle],
  );

  const noteActivity = useCallback(
    (userMessageId: string | null | undefined, activity: AgentActivity) => {
      const target = targetRef.current;
      if (!target) return;
      if (userMessageId && userMessageId !== target) return;
      setAgentActivity(activity);
    },
    [],
  );

  const reconcile = useCallback(
    (isSettled: (userMessageId: string) => boolean) => {
      const target = targetRef.current;
      if (!target) return;
      if (isSettled(target)) clear();
    },
    [clear],
  );

  return {
    inFlight,
    agentActivity,
    begin,
    clear,
    resolveReply,
    resolveError,
    noteActivity,
    reconcile,
  };
}
