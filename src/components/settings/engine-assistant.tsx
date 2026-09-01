import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FieldGroup } from "./primitives";
import { userFacingError } from "@/lib/utils";
import { thisMachine } from "@/lib/platform";

type RelaySettings = {
  relayUrl: string;
  relayToken: string;
  pairedVerified?: boolean;
};

type ProbeResult = {
  reachable: boolean;
  reply?: string | null;
};

type PairStatus = "idle" | "pairing" | "ok" | "error";
type ProbeStatus = "idle" | "checking" | "reachable" | "error";

function relayIsLoopback(raw: string): boolean {
  try {
    const host = new URL(raw.trim()).hostname.replace(/^\[|\]$/g, "").toLowerCase();
    if (host === "localhost" || host.endsWith(".localhost")) return true;
    if (host === "::1") return true;
    return /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(host);
  } catch {
    return false;
  }
}

interface ProviderInfo {
  kind: string;
  human_name: string;
  install_command: string;
  pair_code_command: string;
}

const PROVIDERS: ProviderInfo[] = [
  {
    kind: "openclaw",
    human_name: "OpenClaw",
    install_command: "openclaw plugins install @stagewhisper/stagewhisper",
    pair_code_command: "openclaw stagewhisper pair-code",
  },
  {
    kind: "hermes",
    human_name: "Hermes",
    install_command: "pipx install hermes-platform-stagewhisper && stagewhisper-hermes-install",
    pair_code_command: "stagewhisper-hermes pair-code",
  },
];

function relayHost(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
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
        <code className="block rounded-md bg-card px-3 py-2 pr-10 text-sm text-heading select-text break-all font-mono">
          {command}
        </code>
        <CopyButton text={command} />
      </div>
    </div>
  );
}

export function EngineAssistantCard() {
  const [loaded, setLoaded] = useState(false);
  const [relayUrl, setRelayUrl] = useState("");
  const [relayToken, setRelayToken] = useState("");
  const [verified, setVerified] = useState(false);
  const [revealToken, setRevealToken] = useState(false);
  const [provider, setProvider] = useState<ProviderInfo>(PROVIDERS[0]);
  const [topology, setTopology] = useState<"local" | "remote">("local");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [pairCode, setPairCode] = useState("");
  const [pairStatus, setPairStatus] = useState<PairStatus>("idle");
  const [pairError, setPairError] = useState<string | null>(null);
  const [testError, setTestError] = useState<string | null>(null);
  const [probeStatus, setProbeStatus] = useState<ProbeStatus>("idle");
  const [probeError, setProbeError] = useState<string | null>(null);
  const [probeReply, setProbeReply] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const settings = await invoke<RelaySettings>("get_relay_settings");
      setRelayUrl(settings.relayUrl ?? "");
      setRelayToken(settings.relayToken ?? "");
      setVerified(Boolean(settings.pairedVerified));
      const url = settings.relayUrl ?? "";
      if (url.trim().length > 0) {
        setTopology(relayIsLoopback(url) ? "local" : "remote");
      }
    } catch (err) {
      console.error("[engine-assistant] failed to load", err);
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
      const result = await invoke<ProbeResult>("probe_agent_pairing");
      setProbeReply(result.reply ?? null);
      setProbeStatus("reachable");
    } catch (err) {
      setProbeReply(null);
      setProbeStatus("error");
      setProbeError(userFacingError(err, "Couldn't reach your assistant. Check that it's running and try again."));
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
      setProbeError(
        userFacingError(err, "Couldn't confirm the approval. Make sure you approved this device on the host, then try again."),
      );
    }
  }, []);

  const applyRelaySettings = useCallback((next: RelaySettings) => {
    const url = next.relayUrl ?? "";
    setRelayUrl(url);
    setRelayToken(next.relayToken ?? "");
    setVerified(Boolean(next.pairedVerified));
    if (url.trim().length > 0) {
      setTopology(relayIsLoopback(url) ? "local" : "remote");
    }
  }, []);

  const handlePair = useCallback(async () => {
    setPairStatus("pairing");
    setPairError(null);
    try {
      const next = await invoke<RelaySettings>("pair_with_code", { code: pairCode });
      applyRelaySettings(next);
      setPairStatus("ok");
      setPairCode("");
    } catch (err) {
      setPairStatus("error");
      setPairError(userFacingError(err, "Couldn't pair with that code. Check the code and try again."));
    }
  }, [pairCode, applyRelaySettings]);

  const handleSaveManual = useCallback(async () => {
    setTestError(null);
    try {
      const next = await invoke<RelaySettings>("save_relay_settings", {
        args: { relay_url: relayUrl, relay_token: relayToken },
      });
      applyRelaySettings(next);
    } catch (err) {
      setTestError(userFacingError(err, "Couldn't save those settings. Check the URL and token, then try again."));
    }
  }, [relayUrl, relayToken, applyRelaySettings]);

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
      setTestError(userFacingError(err, "Couldn't disconnect. Try again."));
    }
  }, []);

  const paired = relayUrl.trim().length > 0 && relayToken.trim().length > 0;

  useEffect(() => {
    if (loaded && paired && !verified && probeStatus === "idle") {
      void runProbe();
    }
  }, [loaded, paired, verified, probeStatus, runProbe]);

  if (!loaded) {
    return <p className="px-1 text-sm text-muted">Loading...</p>;
  }

  if (verified) {
    return (
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
              <span className="text-xs text-success">Assistant replied</span>
            )}
            {probeStatus === "error" && <span className="text-xs text-danger">{probeError}</span>}
          </div>
        </div>

        <button
          onClick={() => void handleDisconnect()}
          className="rounded-lg border border-danger/30 px-4 py-2 text-sm font-medium text-danger transition hover:bg-danger-bg"
        >
          Disconnect
        </button>
      </div>
    );
  }

  if (paired) {
    return (
      <div className="space-y-5">
        {probeStatus === "error" ? (
          <p className="text-sm leading-relaxed text-body">
            Couldn't reach your assistant. Make sure it's running, and if it's on another machine,
            that you can reach it at that address. Then check again.
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
            {probeReply ? (
              <>
                <p className="text-xs font-medium text-accent">
                  {`Your ${provider.human_name} assistant replied. If it asks you to approve this device, do that on the host, then confirm below:`}
                </p>
                <div className="group relative">
                  <code className="block whitespace-pre-wrap rounded-md bg-card px-3 py-2 pr-10 text-sm text-heading select-text break-words font-mono">
                    {probeReply}
                  </code>
                  <CopyButton text={probeReply} />
                </div>
              </>
            ) : (
              <p className="text-xs font-medium text-accent">
                {`Checking with your ${provider.human_name} assistant...`}
              </p>
            )}
          </div>
        )}

        {probeStatus === "error" && probeError && (
          <p className="text-sm text-danger">{probeError}</p>
        )}

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
    );
  }

  return (
    <div className="space-y-6">
      <div>
        <p className="text-sm font-medium text-heading">Where does your assistant run?</p>
        <div className="mt-2 flex gap-2">
          {(
            [
              ["local", `On ${thisMachine}`],
              ["remote", "On a VPS / remote host"],
            ] as const
          ).map(([value, text]) => (
            <button
              key={value}
              onClick={() => setTopology(value)}
              className={`rounded-lg border px-4 py-2 text-sm font-medium transition ${
                topology === value
                  ? "border-accent/40 bg-accent/10 text-accent"
                  : "border-rule-strong text-muted hover:border-accent/30 hover:text-body"
              }`}
            >
              {text}
            </button>
          ))}
        </div>
        <p className="mt-2 text-xs leading-relaxed text-muted">
          {topology === "local"
            ? `The assistant runs on ${thisMachine}. Paste the pairing code and it connects automatically.`
            : "The assistant runs on another machine you reach over a private network (for example Tailscale). Point StageWhisper at that address and paste the pairing code."}
        </p>
      </div>

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
        label={
          topology === "local"
            ? `2. Generate a pairing code on your ${provider.human_name} host:`
            : `2. Generate a pairing code on the host, pointing at the address ${thisMachine} reaches it on:`
        }
        command={
          topology === "local"
            ? provider.pair_code_command
            : `${provider.pair_code_command} --url https://your-host.tailnet-name.ts.net`
        }
      />
      {topology === "remote" && (
        <p className="-mt-2 text-xs leading-relaxed text-muted">
          Use your host's Tailscale address (run{" "}
          <code className="font-mono text-body">tailscale serve status</code> on it), not localhost.
          The default code points at 127.0.0.1, which only works on the same machine.
        </p>
      )}

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
          {pairStatus === "ok" && (
            <span className="text-xs text-success">Paired and reachable</span>
          )}
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
              hint={
                topology === "local"
                  ? `HTTP(S) endpoint exposed by your assistant on ${thisMachine}.`
                  : `The address ${thisMachine} reaches your assistant on, e.g. your Tailscale host.`
              }
            >
              <input
                type="text"
                value={relayUrl}
                onChange={(e) => setRelayUrl(e.target.value)}
                placeholder={
                  topology === "local"
                    ? "http://127.0.0.1:8765"
                    : "https://your-host.tailnet-name.ts.net"
                }
                className="w-full rounded-lg border border-rule-strong bg-page px-3 py-2 text-sm text-body outline-none focus:border-accent"
              />
            </FieldGroup>

            <FieldGroup label="Bearer token" hint="The token printed inside the pairing code.">
              <div className="flex items-center gap-2">
                <input
                  type={revealToken ? "text" : "password"}
                  value={relayToken}
                  onChange={(e) => setRelayToken(e.target.value)}
                  placeholder="paste token"
                  autoComplete="off"
                  spellCheck={false}
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
  );
}
