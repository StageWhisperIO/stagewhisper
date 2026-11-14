import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FieldGroup } from "./primitives";
import {
  CommandCard,
  CopyButton,
  PROVIDERS,
  relayHost,
  relayIsLoopback,
  type ProviderInfo,
} from "./engine-assistant-parts";
import { userFacingError } from "@/lib/utils";
import { platformContext } from "@/lib/platform";
import { Trans, t as translate, useTranslation } from "@/i18n";

type RelaySettings = {
  relayUrl: string;
  hasRelayToken?: boolean;
  pairedVerified?: boolean;
};

type ProbeResult = {
  reachable: boolean;
  reply?: string | null;
};

type PairStatus = "idle" | "pairing" | "ok" | "error";
type ProbeStatus = "idle" | "checking" | "reachable" | "error";

export function EngineAssistantCard() {
  const { t } = useTranslation("assistant");
  const onPlatform = { context: platformContext };
  const [loaded, setLoaded] = useState(false);
  const [relayUrl, setRelayUrl] = useState("");
  const [relayToken, setRelayToken] = useState("");
  const [tokenSaved, setTokenSaved] = useState(false);
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
      setRelayToken("");
      setTokenSaved(Boolean(settings.hasRelayToken));
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
      setProbeError(userFacingError(err, translate("assistant:errors.probeFailed")));
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
        userFacingError(err, translate("assistant:errors.approveFailed")),
      );
    }
  }, []);

  const applyRelaySettings = useCallback((next: RelaySettings) => {
    const url = next.relayUrl ?? "";
    setRelayUrl(url);
    setRelayToken("");
    setTokenSaved(Boolean(next.hasRelayToken));
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
      setPairError(userFacingError(err, translate("assistant:errors.pairFailed")));
    }
  }, [pairCode, applyRelaySettings]);

  const handleSaveManual = useCallback(async () => {
    setTestError(null);
    try {
      const next = await invoke<RelaySettings>("save_relay_settings", {
        args: { relay_url: relayUrl, relay_token: relayToken.trim() ? relayToken : null },
      });
      applyRelaySettings(next);
    } catch (err) {
      setTestError(userFacingError(err, translate("assistant:errors.saveFailed")));
    }
  }, [relayUrl, relayToken, applyRelaySettings]);

  const handleDisconnect = useCallback(async () => {
    try {
      await invoke<RelaySettings>("save_relay_settings", {
        args: { relay_url: "", relay_token: "" },
      });
      setRelayUrl("");
      setRelayToken("");
      setTokenSaved(false);
      setVerified(false);
      setPairStatus("idle");
      setProbeStatus("idle");
      setProbeError(null);
    } catch (err) {
      setTestError(userFacingError(err, translate("assistant:errors.disconnectFailed")));
    }
  }, []);

  const paired = relayUrl.trim().length > 0 && tokenSaved;

  useEffect(() => {
    if (loaded && paired && !verified && probeStatus === "idle") {
      void runProbe();
    }
  }, [loaded, paired, verified, probeStatus, runProbe]);

  if (!loaded) {
    return <p className="px-1 text-sm text-muted">{t("loading")}</p>;
  }

  if (verified) {
    return (
      <div className="space-y-6">
        <div className="flex items-center justify-between rounded-lg border border-rule-strong bg-panel px-4 py-3">
          <div className="flex items-center gap-2">
            <span className="h-2 w-2 rounded-full bg-success shadow-[0_0_6px_var(--st-success)]" />
            <span className="text-sm font-medium text-heading">{t("connected.status")}</span>
            <span className="text-xs text-muted">{relayHost(relayUrl)}</span>
          </div>
          <div className="flex items-center gap-3">
            <button
              onClick={() => void runProbe()}
              disabled={probeStatus === "checking"}
              className="rounded-lg border border-rule-strong px-3 py-1.5 text-xs font-medium text-body transition hover:border-accent hover:text-accent disabled:opacity-50"
            >
              {probeStatus === "checking" ? t("connected.testing") : t("connected.test")}
            </button>
            {probeStatus === "reachable" && (
              <span className="text-xs text-success">{t("connected.replied")}</span>
            )}
            {probeStatus === "error" && <span className="text-xs text-danger">{probeError}</span>}
          </div>
        </div>

        <button
          onClick={() => void handleDisconnect()}
          className="rounded-lg border border-danger/30 px-4 py-2 text-sm font-medium text-danger transition hover:bg-danger-bg"
        >
          {t("connected.disconnect")}
        </button>
      </div>
    );
  }

  if (paired) {
    return (
      <div className="space-y-5">
        {probeStatus === "error" ? (
          <p className="text-sm leading-relaxed text-body">
            {t("approval.unreachableBody")}
          </p>
        ) : (
          <p className="text-sm leading-relaxed text-body">
            {t("approval.approveBody")}
          </p>
        )}

        {probeStatus === "checking" && (
          <p className="text-sm text-muted">{t("approval.checkingConnection")}</p>
        )}

        {probeStatus !== "error" && (
          <div className="rounded-lg border border-accent/20 bg-accent/5 px-4 py-3 space-y-1">
            {probeReply ? (
              <>
                <p className="text-xs font-medium text-accent">
                  {t("approval.replied", { provider: provider.human_name })}
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
                {t("approval.checkingWith", { provider: provider.human_name })}
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
              {probeStatus === "error" ? t("approval.unreachable") : t("approval.needed")}
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
            {probeStatus === "checking" ? t("approval.verifying") : t("approval.confirm")}
          </button>
          <button
            onClick={() => void runProbe()}
            disabled={probeStatus === "checking"}
            className="rounded-lg border border-rule-strong px-4 py-2 text-sm font-medium text-body transition hover:border-accent hover:text-accent disabled:opacity-50"
          >
            {probeStatus === "checking" ? t("approval.checking") : t("approval.check")}
          </button>
          <button
            onClick={() => void handleDisconnect()}
            className="text-xs font-medium text-muted transition hover:text-danger"
          >
            {t("approval.startOver")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-6">
      <div>
        <p className="text-sm font-medium text-heading">{t("setup.where")}</p>
        <div className="mt-2 flex gap-2">
          {(
            [
              ["local", t("setup.local", onPlatform)],
              ["remote", t("setup.remote")],
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
            ? t("setup.localHint", onPlatform)
            : t("setup.remoteHint")}
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
        label={t("setup.install", { provider: provider.human_name })}
        command={provider.install_command}
      />
      {provider.install_note_key && (
        <p className="-mt-2 text-xs leading-relaxed text-muted">{t(provider.install_note_key)}</p>
      )}

      <CommandCard
        label={
          provider.needs_host_address && topology === "remote"
            ? t("setup.pairCodeRemote", onPlatform)
            : t("setup.pairCode", { provider: provider.human_name })
        }
        command={
          provider.needs_host_address && topology === "remote"
            ? `${provider.pair_code_command} --url https://your-host.tailnet-name.ts.net`
            : provider.pair_code_command
        }
      />
      {provider.needs_host_address && topology === "remote" && (
        <p className="-mt-2 text-xs leading-relaxed text-muted">
          <Trans
            t={t}
            i18nKey="setup.tailscaleHint"
            components={{ code: <code className="font-mono text-body" /> }}
          />
        </p>
      )}
      {provider.pair_code_note_key && (
        <p className="-mt-2 text-xs leading-relaxed text-muted">{t(provider.pair_code_note_key)}</p>
      )}

      <div className="space-y-3">
        <FieldGroup
          label={t("setup.pasteCode")}
          hint={t("setup.pasteCodeHint")}
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
            {pairStatus === "pairing" ? t("setup.pairing") : t("setup.pair")}
          </button>
          {pairStatus === "ok" && (
            <span className="text-xs text-success">{t("setup.paired")}</span>
          )}
          {pairStatus === "error" && <span className="text-xs text-danger">{pairError}</span>}
        </div>
      </div>

      <p className="text-xs leading-relaxed text-muted">
        {t("setup.afterPairing", { provider: provider.human_name })}
      </p>

      <div className="space-y-3">
        <button
          onClick={() => setShowAdvanced((v) => !v)}
          className="text-xs font-medium text-muted transition hover:text-accent"
        >
          {showAdvanced ? t("manual.hideSetup") : t("manual.showSetup")}
        </button>

        {showAdvanced && (
          <div className="space-y-5">
            <FieldGroup
              label={t("manual.relayUrl")}
              hint={
                topology === "local"
                  ? t("manual.relayUrlHint", onPlatform)
                  : t("manual.relayUrlRemoteHint", onPlatform)
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

            <FieldGroup label={t("manual.token")} hint={t("manual.tokenHint")}>
              <div className="flex items-center gap-2">
                <input
                  type={revealToken ? "text" : "password"}
                  value={relayToken}
                  onChange={(e) => setRelayToken(e.target.value)}
                  placeholder={tokenSaved ? t("manual.tokenSavedPlaceholder") : t("manual.tokenPlaceholder")}
                  autoComplete="off"
                  spellCheck={false}
                  className="flex-1 rounded-lg border border-rule-strong bg-page px-3 py-2 font-mono text-xs text-body outline-none focus:border-accent"
                />
                <button
                  onClick={() => setRevealToken((v) => !v)}
                  className="rounded-lg border border-rule-strong px-3 py-2 text-xs font-medium text-body transition hover:border-accent hover:text-accent"
                >
                  {revealToken ? t("manual.hideToken") : t("manual.revealToken")}
                </button>
              </div>
            </FieldGroup>

            <button
              onClick={() => void handleSaveManual()}
              className="rounded-lg bg-accent px-4 py-2 text-sm font-semibold text-accent-fg transition hover:bg-accent-hover"
            >
              {t("common:actions.save")}
            </button>
            {testError && <span className="ml-3 text-xs text-danger">{testError}</span>}
          </div>
        )}
      </div>
    </div>
  );
}
