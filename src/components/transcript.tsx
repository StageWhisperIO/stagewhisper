import { useCallback, useEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Message } from "../hooks/messages";
import { cn } from "@/lib/utils";

const OVERSCAN = 10;
const ESTIMATED_ROW_HEIGHT = 52;
const BOTTOM_THRESHOLD_PX = 48;

export function TranscriptView({ messages }: { messages: Message[] }) {
  const parentRef = useRef<HTMLDivElement>(null);
  const isStuckRef = useRef(true);

  const virtualizer = useVirtualizer({
    count: messages.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ESTIMATED_ROW_HEIGHT,
    overscan: OVERSCAN,
  });

  const onScroll = useCallback(() => {
    const el = parentRef.current;
    if (!el) return;
    const distanceFromBottom = el.scrollHeight - el.scrollTop - el.clientHeight;
    isStuckRef.current = distanceFromBottom <= BOTTOM_THRESHOLD_PX;
  }, []);

  useEffect(() => {
    if (isStuckRef.current && messages.length > 0) {
      virtualizer.scrollToIndex(messages.length - 1, { align: "end" });
    }
  }, [messages, virtualizer]);

  return (
    <div ref={parentRef} onScroll={onScroll} className="flex h-full flex-col overflow-y-auto">
      {messages.length === 0 ? (
        <div className="flex flex-1 items-center justify-center">
          <p className="text-center text-[12px] leading-relaxed text-[--text-dimmed]">
            Waiting for audio…
          </p>
        </div>
      ) : (
        <div
          className="relative w-full px-5 pb-4 pt-2"
          style={{ height: `${virtualizer.getTotalSize()}px` }}
        >
          {virtualizer.getVirtualItems().map((virtualRow) => {
            const msg = messages[virtualRow.index];
            const prev = virtualRow.index > 0 ? messages[virtualRow.index - 1] : null;
            const next =
              virtualRow.index < messages.length - 1 ? messages[virtualRow.index + 1] : null;
            const isFirst = !prev || prev.kind !== msg.kind || prev.source !== msg.source;
            const isLast = !next || next.kind !== msg.kind || next.source !== msg.source;
            return (
              <div
                key={msg.id}
                data-index={virtualRow.index}
                ref={virtualizer.measureElement}
                className="absolute left-0 w-full"
                style={{ transform: `translateY(${virtualRow.start}px)` }}
              >
                <div className={isFirst ? "mt-1" : ""} style={{ paddingBottom: "6px" }}>
                  <MessageBubble message={msg} isFirst={isFirst} isLast={isLast} />
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}

function formatTime(timestamp: number): string {
  const date = new Date(timestamp);
  return date.toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function MessageBubble({
  message,
  isFirst,
  isLast,
}: {
  message: Message;
  isFirst: boolean;
  isLast: boolean;
}) {
  const isOutput = message.kind === "output";
  const isRight = isOutput || message.source === "you";
  const sourceLabel = message.source === "you" ? "You" : "Others";

  const outputRadius = cn(
    isFirst && isLast && "rounded-[18px]",
    isFirst && !isLast && "rounded-[18px] rounded-br-[6px]",
    !isFirst && !isLast && "rounded-[18px] rounded-r-[6px]",
    !isFirst && isLast && "rounded-[18px] rounded-tr-[6px]",
  );

  const inputRadius = cn(
    isFirst && isLast && "rounded-[18px]",
    isFirst && !isLast && "rounded-[18px] rounded-bl-[6px]",
    !isFirst && !isLast && "rounded-[18px] rounded-l-[6px]",
    !isFirst && isLast && "rounded-[18px] rounded-tl-[6px]",
  );

  return (
    <div className={cn("flex", isRight ? "justify-end" : "justify-start")}>
      <div
        className={cn(
          "max-w-[85%] px-3.5 py-2",
          isRight
            ? cn("bg-[#2563eb]", outputRadius)
            : cn("bg-[--glass-bg-input] border-[0.5px] border-white/10", inputRadius),
        )}
      >
        {isFirst && message.source && (
          <span
            className={cn(
              "mb-0.5 block text-[10px] font-medium",
              isRight ? "text-right text-white/60" : "text-[--text-dimmed]",
            )}
          >
            {sourceLabel}
          </span>
        )}
        <p
          className={cn(
            "text-[13px] font-normal leading-relaxed tracking-[-0.01em]",
            isRight ? "text-white" : "text-[--text-muted]",
          )}
        >
          {message.text}
        </p>
        {isLast && (
          <span
            className={cn(
              "mt-0.5 block text-[10px]",
              isRight ? "text-right text-white/50" : "text-[--text-dimmed]",
            )}
          >
            {formatTime(message.timestamp)}
          </span>
        )}
      </div>
    </div>
  );
}
