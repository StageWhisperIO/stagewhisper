import type { ReactNode } from "react";

function renderInline(text: string, keyPrefix: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  const pattern = /\*\*([^*]+)\*\*|`([^`]+)`/g;
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  let i = 0;
  while ((match = pattern.exec(text)) !== null) {
    if (match.index > lastIndex) {
      nodes.push(text.slice(lastIndex, match.index));
    }
    if (match[1] !== undefined) {
      nodes.push(
        <strong key={`${keyPrefix}-b-${i}`} className="font-semibold text-heading">
          {match[1]}
        </strong>,
      );
    } else if (match[2] !== undefined) {
      nodes.push(
        <code
          key={`${keyPrefix}-c-${i}`}
          className="rounded bg-sidebar-active px-1 py-0.5 font-mono text-[0.85em]"
        >
          {match[2]}
        </code>,
      );
    }
    lastIndex = pattern.lastIndex;
    i += 1;
  }
  if (lastIndex < text.length) {
    nodes.push(text.slice(lastIndex));
  }
  return nodes;
}

export function Markdown({ source }: { source: string }) {
  const lines = source.replace(/\r\n/g, "\n").split("\n");
  const blocks: ReactNode[] = [];
  let listItems: ReactNode[] = [];
  let listKey = 0;

  const flushList = () => {
    if (listItems.length > 0) {
      blocks.push(
        <ul key={`ul-${listKey++}`} className="my-1 space-y-1">
          {listItems}
        </ul>,
      );
      listItems = [];
    }
  };

  lines.forEach((raw, idx) => {
    const line = raw.trimEnd();
    const key = `md-${idx}`;

    const checkbox = line.match(/^\s*[-*]\s+\[( |x|X)\]\s+(.*)$/);
    if (checkbox) {
      listItems.push(
        <li key={key} className="flex items-start gap-2">
          <input
            type="checkbox"
            checked={checkbox[1].toLowerCase() === "x"}
            readOnly
            className="mt-1 h-3.5 w-3.5 shrink-0"
          />
          <span>{renderInline(checkbox[2], key)}</span>
        </li>,
      );
      return;
    }

    const bullet = line.match(/^\s*[-*]\s+(.*)$/);
    if (bullet) {
      listItems.push(
        <li key={key} className="flex items-start gap-2">
          <span className="mt-1 text-muted">•</span>
          <span>{renderInline(bullet[1], key)}</span>
        </li>,
      );
      return;
    }

    flushList();

    if (line.startsWith("### ")) {
      blocks.push(
        <h3 key={key} className="mt-3 text-sm font-semibold text-heading">
          {renderInline(line.slice(4), key)}
        </h3>,
      );
      return;
    }
    if (line.startsWith("## ")) {
      blocks.push(
        <h2 key={key} className="mt-4 text-base font-semibold text-heading">
          {renderInline(line.slice(3), key)}
        </h2>,
      );
      return;
    }
    if (line.startsWith("# ")) {
      blocks.push(
        <h1 key={key} className="mt-2 text-lg font-bold text-heading">
          {renderInline(line.slice(2), key)}
        </h1>,
      );
      return;
    }
    if (line.trim() === "") {
      return;
    }
    blocks.push(
      <p key={key} className="my-1 leading-relaxed">
        {renderInline(line, key)}
      </p>,
    );
  });

  flushList();

  return <div className="text-sm text-body">{blocks}</div>;
}
