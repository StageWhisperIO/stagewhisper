import { useEffect, useRef, useState } from "react";
import { useTranslation } from "@/i18n";

export interface ProviderInfo {
  kind: string;
  human_name: string;
  install_command: string;
  pair_code_command: string;
  needs_host_address: boolean;
  install_note_key?: string;
  pair_code_note_key?: string;
}

export const PROVIDERS: ProviderInfo[] = [
  {
    kind: "openclaw",
    human_name: "OpenClaw",
    install_command: "openclaw plugins install @stagewhisper/stagewhisper",
    pair_code_command: "openclaw stagewhisper pair-code",
    needs_host_address: true,
  },
  {
    kind: "hermes",
    human_name: "Hermes",
    install_command:
      "uv pip install --python <hermes-venv>/bin/python hermes-platform-stagewhisper && <hermes-venv>/bin/stagewhisper-hermes-install",
    pair_code_command: "<hermes-venv>/bin/stagewhisper-hermes pair-code",
    needs_host_address: true,
    install_note_key: "providers.hermes.installNote",
    pair_code_note_key: "providers.hermes.pairCodeNote",
  },
];

export function relayIsLoopback(raw: string): boolean {
  try {
    const host = new URL(raw.trim()).hostname.replace(/^\[|\]$/g, "").toLowerCase();
    if (host === "localhost" || host.endsWith(".localhost")) return true;
    if (host === "::1") return true;
    return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);
  } catch {
    return false;
  }
}

export function relayHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

export function CopyButton({ text }: { text: string }) {
  const { t } = useTranslation("assistant");
  const [copied, setCopied] = useState(false);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    return () => {
      if (timeoutRef.current) clearTimeout(timeoutRef.current);
    };
  }, []);

  const handleClick = () => {
    navigator.clipboard.writeText(text);
    setCopied(true);
    if (timeoutRef.current) clearTimeout(timeoutRef.current);
    timeoutRef.current = setTimeout(() => setCopied(false), 1500);
  };

  const label = copied ? t("common:actions.copied") : t("copy");

  return (
    <button
      type="button"
      onClick={handleClick}
      className="absolute right-2 top-2 rounded-md bg-sidebar-active p-1.5 text-muted opacity-0 transition hover:text-heading group-hover:opacity-100"
      title={label}
      aria-label={label}
    >
      <span className="relative block h-3.5 w-3.5">
        <svg
          xmlns="http://www.w3.org/2000/svg"
          viewBox="0 0 16 16"
          fill="currentColor"
          className={`absolute inset-0 h-3.5 w-3.5 transition-all duration-200 ${
            copied ? "scale-75 opacity-0" : "scale-100 opacity-100"
          }`}
        >
          <path d="M5.75 1a.75.75 0 0 0-.75.75v1.5a.75.75 0 0 0 1.5 0v-.75h5.5v5.5h-.75a.75.75 0 0 0 0 1.5h1.5a.75.75 0 0 0 .75-.75v-7A.75.75 0 0 0 12.75 1h-7Z" />
          <path d="M3.25 6a.75.75 0 0 0-.75.75v7c0 .414.336.75.75.75h7a.75.75 0 0 0 .75-.75v-7a.75.75 0 0 0-.75-.75h-7Zm.75 7V7.5h5.5V13H4Z" />
        </svg>
        <svg
          xmlns="http://www.w3.org/2000/svg"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          className={`absolute inset-0 h-3.5 w-3.5 text-success transition-all duration-200 ${
            copied ? "scale-100 opacity-100" : "scale-75 opacity-0"
          }`}
        >
          <path d="M3.5 8.5 6.5 11.5 12.5 5" />
        </svg>
      </span>
    </button>
  );
}

export function CommandCard({ label, command }: { label: string; command: string }) {
  return (
    <div className="rounded-lg bg-accent/5 border border-accent/20 px-4 py-3 space-y-1">
      <p className="text-xs font-medium text-accent">{label}</p>
      <div className="group relative">
        <code className="block rounded-md bg-card px-3 py-2 pr-10 text-sm text-heading select-text break-all font-mono">
          {command}
        </code>
        <CopyButton text={command} />
      </div>
    </div>
  );
}
