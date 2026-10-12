import { Channel, invoke } from "@tauri-apps/api/core";
import type { ChatTransport, UIMessage, UIMessageChunk } from "ai";

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

export interface ChatMessageMetadata {
  createdAt: string;
  status: string;
  parentMessageId: string | null;
  errorCode: string | null;
  errorMessage: string | null;
}

export type ChatDataParts = {
  activity: { label: string };
};

export type ChatUIMessage = UIMessage<ChatMessageMetadata, ChatDataParts>;

export function chatMessageToUI(message: ChatMsg): ChatUIMessage {
  return {
    id: message.id,
    role: message.role === "user" ? "user" : "assistant",
    metadata: {
      createdAt: message.created_at,
      status: message.status,
      parentMessageId: message.parent_message_id ?? null,
      errorCode: message.error_code ?? null,
      errorMessage: message.error_message ?? null,
    },
    parts: [{ type: "text", text: message.content }],
  };
}

export function uiMessageText(message: ChatUIMessage): string {
  return message.parts
    .filter((part) => part.type === "text")
    .map((part) => part.text)
    .join("");
}

export function lastUserMessage(messages: ChatUIMessage[]): ChatUIMessage | undefined {
  return [...messages].reverse().find((message) => message.role === "user");
}

function wireChunkChannel(
  onChunk: Channel<UIMessageChunk>,
  controller: ReadableStreamDefaultController<UIMessageChunk>,
) {
  let closed = false;
  onChunk.onmessage = (chunk) => {
    if (closed) return;
    try {
      controller.enqueue(chunk);
    } catch {
      closed = true;
      return;
    }
    if (chunk.type === "finish" || chunk.type === "error") {
      closed = true;
      try {
        controller.close();
      } catch {}
    }
  };
  return {
    fail(err: unknown) {
      if (closed) return;
      closed = true;
      controller.error(err instanceof Error ? err : new Error(String(err)));
    },
  };
}

function createBufferedChunkRelay(onChunk: Channel<UIMessageChunk>) {
  const buffered: UIMessageChunk[] = [];
  let controller: ReadableStreamDefaultController<UIMessageChunk> | null = null;
  let closed = false;

  function deliver(chunk: UIMessageChunk) {
    if (closed || !controller) return;
    try {
      controller.enqueue(chunk);
    } catch {
      closed = true;
      return;
    }
    if (chunk.type === "finish" || chunk.type === "error") {
      closed = true;
      try {
        controller.close();
      } catch {}
    }
  }

  onChunk.onmessage = (chunk) => {
    if (controller) {
      deliver(chunk);
    } else {
      buffered.push(chunk);
    }
  };

  return {
    attach(readyController: ReadableStreamDefaultController<UIMessageChunk>) {
      controller = readyController;
      const queued = buffered.splice(0, buffered.length);
      for (const chunk of queued) {
        deliver(chunk);
      }
    },
  };
}

export class TauriChatTransport implements ChatTransport<ChatUIMessage> {
  constructor(private readonly sessionId: string) {}

  async sendMessages({
    messages,
    abortSignal,
  }: Parameters<ChatTransport<ChatUIMessage>["sendMessages"]>[0]) {
    const last = lastUserMessage(messages);
    const text = last ? uiMessageText(last).trim() : "";
    if (!text) throw new Error("Write a message first.");
    if (abortSignal?.aborted) throw new DOMException("Aborted", "AbortError");

    const sessionId = this.sessionId;
    const turnId = crypto.randomUUID();

    return new ReadableStream<UIMessageChunk>({
      start: async (controller) => {
        const onChunk = new Channel<UIMessageChunk>();
        const wired = wireChunkChannel(onChunk, controller);
        abortSignal?.addEventListener("abort", () => {
          void invoke("cancel_session_chat_turn", { turnId }).catch(() => {});
        });
        try {
          await invoke("stream_session_chat_message", { sessionId, text, turnId, onChunk });
        } catch (err) {
          wired.fail(err);
        }
      },
    });
  }

  async reconnectToStream() {
    const onChunk = new Channel<UIMessageChunk>();
    const relay = createBufferedChunkRelay(onChunk);
    const resumed = await invoke<boolean>("resume_session_chat_turn", {
      sessionId: this.sessionId,
      onChunk,
    });
    if (!resumed) return null;
    return new ReadableStream<UIMessageChunk>({
      start: (controller) => {
        relay.attach(controller);
      },
    });
  }
}
