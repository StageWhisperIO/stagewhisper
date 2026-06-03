import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FieldGroup } from "./primitives";

type RelaySettings = {
  relayUrl: string;
  relayToken: string;
  pairedVerified?: boolean;
};

type ProbeResult = {
  reachable: boolean;
};

type PairStatus = "idle" | "pairing" | "ok" | "error";
type ProbeStatus = "idle" | "checking" | "reachable" | "error";

interface ProviderInfo {
  kind: string;
  human_name: string;
  install_command: string;
  pair_code_command: string;
  approve_command: string;
}

const PROVIDERS: ProviderInfo[] = [
  {
    kind: "openclaw",
    human_name: "OpenClaw",
    install_command: "openclaw plugins install @stagewhisper/stagewhisper",
    pair_code_command: "openclaw stagewhisper pair-code",
    approve_command: "openclaw pairing approve stagewhisper <code>",
  },
  {
    kind: "hermes",
    human_name: "Hermes",
    install_command: "pipx install hermes-platform-stagewhisper && stagewhisper-hermes-install",
    pair_code_command: "stagewhisper-hermes pair-code",
    approve_command: "hermes pairing approve stagewhisper <code>",
  },
];

function relayHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

function maskToken(token: string): string {
  if (!token) return "";
  if (token.length <= 4) return "*".repeat(token.length);
  return "*".repeat(token.length - 4) + token.slice(-4);
}

function CopyButton({ text }: { text: string }) {
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

  return (
    <button
      type="button"
      onClick={handleClick}
      className="absolute right-2 top-2 rounded-md bg-sidebar-active p-1.5 text-muted opacity-0 transition hover:text-heading group-hover:opacity-100"
      title={copied ? "Copied" : "Copy to clipboard"}
      aria-label={copied ? "Copied" : "Copy to clipboard"}
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

function CommandCard({ label, command }: { label: string; command: string }) {
  return (
    <div className="rounded-lg bg-accent/5 border border-accent/20 px-4 py-3 space-y-1">
      <p className="text-xs font-medium text-accent">{label}</p>
      <div className="group relative">
        <code className="block rounded-md bg-card px-3 py-2 pr-10 text-sm text-heading select-all break-all font-mono">
          {command}
        </code>
        <CopyButton text={command} />
      </div>
    </div>
  );
}

export function ConnectionSection() {
  const [loaded, setLoaded] = useState(false);
  const [relayUrl, setRelayUrl] = useState("");
  const [relayToken, setRelayToken] = useState("");
  const [verified, setVerified] = useState(false);
  const [revealToken, setRevealToken] = useState(false);
  const [provider, setProvider] = useState<ProviderInfo>(PROVIDERS[0]);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [pairCode, setPairCode] = useState("");
  const [pairStatus, setPairStatus] = useState<PairStatus>("idle");
  const [pairError, setPairError] = useState<string | null>(null);
  const [testError, setTestError] = useState<string | null>(null);
  const [probeStatus, setProbeStatus] = useState<ProbeStatus>("idle");
  const [probeError, setProbeError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const settings = await invoke<RelaySettings>("get_relay_settings");
      setRelayUrl(settings.relayUrl ?? "");
      setRelayToken(settings.relayToken ?? "");
      setVerified(Boolean(settings.pairedVerified));
    } catch (err) {
      console.error("[connection] failed to load", err);
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const runProbe = useCallback(async () => {
    setProbeStatus("checking");
    setProbeError(null);
    try {
      await invoke<ProbeResult>("probe_agent_pairing");
      setProbeStatus("reachable");
    } catch (err) {
      setProbeStatus("error");
      setProbeError(String(err));
    }
  }, []);

  const handleApprove = useCallback(async () => {
    setProbeStatus("checking");
    setProbeError(null);
    try {
      const next = await invoke<RelaySettings>("confirm_device_approved");
      setVerified(Boolean(next.pairedVerified));
      setProbeStatus("idle");
    } catch (err) {
      setProbeStatus("error");
      setProbeError(String(err));
    }
  }, []);

  const handlePair = useCallback(async () => {
    setPairStatus("pairing");
    setPairError(null);
    try {
      const next = await invoke<RelaySettings>("pair_with_code", { code: pairCode });
      setRelayUrl(next.relayUrl ?? "");
      setRelayToken(next.relayToken ?? "");
      setVerified(Boolean(next.pairedVerified));
      setPairStatus("ok");
      setPairCode("");
      await runProbe();
    } catch (err) {
      setPairStatus("error");
      setPairError(String(err));
    }
  }, [pairCode, runProbe]);

  const handleSaveManual = useCallback(async () => {
    setTestError(null);
    try {
      const next = await invoke<RelaySettings>("save_relay_settings", {
        args: { relay_url: relayUrl, relay_token: relayToken },
      });
      setRelayUrl(next.relayUrl ?? "");
      setRelayToken(next.relayToken ?? "");
      setVerified(Boolean(next.pairedVerified));
      await runProbe();
    } catch (err) {
      setTestError(String(err));
    }
  }, [relayUrl, relayToken, runProbe]);

  const handleDisconnect = useCallback(async () => {
    try {
      await invoke<RelaySettings>("save_relay_settings", {
        args: { relay_url: "", relay_token: "" },
      });
      setRelayUrl("");
      setRelayToken("");
      setVerified(false);
      setPairStatus("idle");
      setProbeStatus("idle");
      setProbeError(null);
    } catch (err) {
      setTestError(String(err));
    }
  }, []);

  const paired = relayUrl.trim().length > 0 && relayToken.trim().length > 0;

  useEffect(() => {
    if (loaded && paired && !verified && probeStatus === "idle") {
      void runProbe();
    }
  }, [loaded, paired, verified, probeStatus, runProbe]);

  if (!loaded) {
    return (
      <main className="flex h-full w-full items-center justify-center px-8 text-sm text-muted">
        Loading...
      </main>
    );
  }

  const approveCommand = provider.approve_command;

  return (
    <main className="flex h-full w-full flex-col overflow-y-auto px-8 pb-8 pt-2 text-body">
      <section className="space-y-6">
        <header>
          <h2 className="text-lg font-semibold text-heading">Connection</h2>
          <p className="mt-1 text-sm text-muted">
            After each call, StageWhisper sends the transcript to your own AI assistant over a
            local relay. No cloud round-trip.
          </p>
        </header>

        {verified ? (
          <div className="space-y-6">
            <div className="flex items-center justify-between rounded-lg border border-rule-strong bg-panel px-4 py-3">
              <div className="flex items-center gap-2">
                <span className="h-2 w-2 rounded-full bg-success shadow-[0_0_6px_var(--st-success)]" />
                <span className="text-sm font-medium text-heading">Connected</span>
                <span className="text-xs text-muted">{relayHost(relayUrl)}</span>
              </div>
              <div className="flex items-center gap-3">
                <button
                  onClick={() => void runProbe()}
                  disabled={probeStatus === "checking"}
                  className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent disabled:opacity-50"
                >
                  {probeStatus === "checking" ? "Testing..." : "Test"}
                </button>
                {probeStatus === "reachable" && (
                  <span className="text-xs text-success">Relay reachable</span>
                )}
                {probeStatus === "error" && <span className="text-xs text-danger">{probeError}</span>}
              </div>
            </div>

            <div className="border-t border-rule pt-5">
              <button
                onClick={() => void handleDisconnect()}
                className="rounded-lg border border-danger/30 px-4 py-2 text-sm font-medium text-danger transition hover:bg-danger-bg"
              >
                Disconnect
              </button>
            </div>
          </div>
        ) : paired ? (
          <div className="space-y-5">
            <div className="flex items-center justify-between rounded-lg border border-rule-strong bg-panel px-4 py-3">
              <div className="flex items-center gap-2">
                <span
                  className={`h-2 w-2 rounded-full ${
                    probeStatus === "error"
                      ? "bg-danger shadow-[0_0_6px_var(--st-danger)]"
                      : "bg-accent shadow-[0_0_6px_var(--st-accent)]"
                  }`}
                />
                <span className="text-sm font-medium text-heading">
                  {probeStatus === "error" ? "Can't reach your assistant" : "Approval needed"}
                </span>
                <span className="text-xs text-muted">{relayHost(relayUrl)}</span>
              </div>
            </div>

            {probeStatus === "error" ? (
              <p className="text-sm leading-relaxed text-body">
                Couldn't reach your assistant relay. Make sure the assistant is running and, if
                it's on a remote host, that your tunnel is up. Then check again.
              </p>
            ) : (
              <p className="text-sm leading-relaxed text-body">
                Approve this device once on your assistant host, then confirm below. Until then it
                can't respond, so recording stays disabled.
              </p>
            )}

            {probeStatus === "checking" && (
              <p className="text-sm text-muted">Checking the connection...</p>
            )}

            {probeStatus !== "error" && (
              <div className="rounded-lg border border-accent/20 bg-accent/5 px-4 py-3 space-y-1">
                <p className="text-xs font-medium text-accent">
                  {`Run this on your ${provider.human_name} host (use the code it shows):`}
                </p>
                <div className="group relative">
                  <code className="block rounded-md bg-card px-3 py-2 pr-10 text-sm text-heading select-all break-all font-mono">
                    {approveCommand}
                  </code>
                  <CopyButton text={approveCommand} />
                </div>
              </div>
            )}

            {probeStatus === "error" && probeError && (
              <p className="text-sm text-danger">{probeError}</p>
            )}

            <div className="flex items-center gap-3">
              <button
                onClick={() => void handleApprove()}
                disabled={probeStatus === "checking"}
                className="rounded-lg bg-accent px-5 py-2.5 text-sm font-medium text-accent-fg transition hover:bg-accent-hover disabled:opacity-50"
              >
                {probeStatus === "checking" ? "Verifying..." : "I've approved this device"}
              </button>
              <button
                onClick={() => void runProbe()}
                disabled={probeStatus === "checking"}
                className="rounded-lg border border-rule-strong px-4 py-2 text-sm font-medium text-body transition hover:border-accent hover:text-accent disabled:opacity-50"
              >
                {probeStatus === "checking" ? "Checking..." : "Check connection"}
              </button>
              <button
                onClick={() => void handleDisconnect()}
                className="text-xs font-medium text-muted transition hover:text-danger"
              >
                Start over
              </button>
            </div>
          </div>
        ) : (
          <div className="space-y-5">
            <p className="text-sm leading-relaxed text-body">
              {`Pair StageWhisper with your ${provider.human_name} assistant so it can summarise each call and draft action items afterwards.`}
            </p>

            <div className="flex gap-2">
              {PROVIDERS.map((p) => (
                <button
                  key={p.kind}
                  onClick={() => setProvider(p)}
                  className={`rounded-lg border px-4 py-2 text-sm font-medium transition ${
                    provider.kind === p.kind
                      ? "border-accent/40 bg-accent/10 text-accent"
                      : "border-rule-strong text-muted hover:border-accent/30 hover:text-body"
                  }`}
                >
                  {p.human_name}
                </button>
              ))}
            </div>

            <CommandCard
              label={`1. Install the StageWhisper plugin on your ${provider.human_name} host:`}
              command={provider.install_command}
            />

            <CommandCard
              label={`2. Generate a pairing code on your ${provider.human_name} host:`}
              command={provider.pair_code_command}
            />

            <div className="space-y-3">
              <FieldGroup
                label="3. Paste the pairing code it prints"
                hint="The command outputs a stagewhisper-pair code. Copy it and paste it here."
              >
                <textarea
                  value={pairCode}
                  onChange={(e) => setPairCode(e.target.value)}
                  placeholder="stagewhisper-pair:v1:..."
                  rows={3}
                  className="w-full resize-none rounded-lg border border-rule-strong bg-page px-3 py-2 font-mono text-xs text-body outline-none focus:border-accent"
                />
              </FieldGroup>
              <div className="flex items-center gap-3">
                <button
                  onClick={() => void handlePair()}
                  disabled={pairStatus === "pairing" || pairCode.trim().length === 0}
                  className="rounded-lg bg-accent px-5 py-2.5 text-sm font-medium text-accent-fg transition hover:bg-accent-hover disabled:opacity-50"
                >
                  {pairStatus === "pairing" ? "Pairing..." : "Pair"}
                </button>
                {pairStatus === "ok" && <span className="text-xs text-success">Paired and reachable</span>}
                {pairStatus === "error" && <span className="text-xs text-danger">{pairError}</span>}
              </div>
            </div>

            <p className="text-xs leading-relaxed text-muted">
              {`After pairing, your ${provider.human_name} assistant approves this device once before it starts responding. We'll walk you through that next.`}
            </p>

            <div className="space-y-3">
              <button
                onClick={() => setShowAdvanced((v) => !v)}
                className="text-xs font-medium text-muted transition hover:text-accent"
              >
                {showAdvanced ? "Hide manual setup" : "Enter relay URL and token manually"}
              </button>

              {showAdvanced && (
                <div className="space-y-5">
                  <FieldGroup
                    label="Relay URL"
                    hint="HTTP(S) endpoint exposed by your assistant integration."
                  >
                    <input
                      type="text"
                      value={relayUrl}
                      onChange={(e) => setRelayUrl(e.target.value)}
                      placeholder="http://127.0.0.1:8765"
                      className="w-full rounded-lg border border-rule-strong bg-page px-3 py-2 text-sm text-body outline-none focus:border-accent"
                    />
                  </FieldGroup>

                  <FieldGroup label="Bearer token" hint="Generated by the integration at install time.">
                    <div className="flex items-center gap-2">
                      <input
                        type={revealToken ? "text" : "password"}
                        value={revealToken ? relayToken : maskToken(relayToken)}
                        onChange={(e) => {
                          if (revealToken) setRelayToken(e.target.value);
                        }}
                        placeholder="paste token"
                        className="flex-1 rounded-lg border border-rule-strong bg-page px-3 py-2 font-mono text-xs text-body outline-none focus:border-accent"
                      />
                      <button
                        onClick={() => setRevealToken((v) => !v)}
                        className="rounded-lg border border-rule-strong px-3 py-2 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
                      >
                        {revealToken ? "Hide" : "Reveal"}
                      </button>
                    </div>
                  </FieldGroup>

                  <button
                    onClick={() => void handleSaveManual()}
                    className="rounded-lg bg-accent px-4 py-2 text-sm font-semibold text-accent-fg transition hover:bg-accent-hover"
                  >
                    Save
                  </button>
                  {testError && <span className="ml-3 text-xs text-danger">{testError}</span>}
                </div>
              )}
            </div>
          </div>
        )}
      </section>
    </main>
  );
}
