import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";

export function AboutSection() {
  const [appVersion, setAppVersion] = useState<string | null>(null);

  useEffect(() => {
    getVersion()
      .then(setAppVersion)
      .catch(() => {});
  }, []);

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="border-b border-rule px-6 py-5">
        <h2 className="text-base font-semibold text-heading">About</h2>
        <p className="mt-0.5 text-sm text-muted">StageWhisper Free</p>
      </header>

      <div className="settings-scroll flex-1 overflow-y-auto px-6 py-5 space-y-5">
        <section className="space-y-3">
          <p className="text-sm leading-relaxed text-body">
            Most AI notetakers wait until the call is over, then send you a summary of what already
            happened, written by a generic model in someone else's cloud.
          </p>
          <p className="text-sm leading-relaxed text-body">
            StageWhisper hands the transcript to your own assistant instead. OpenClaw, Hermes,
            whatever you already run. The one that knows your work writes the summary and the action
            items, not a generic model you have never trained.
          </p>
        </section>

        <div className="border-t border-rule pt-5 space-y-3">
          <div>
            <p className="text-sm font-medium text-heading">How it works</p>
            <p className="mt-1 text-xs leading-relaxed text-muted">
              Transcription runs here, on this device, while you talk. When the call ends the full
              transcript goes once to your own AI over the relay you configured and comes back as a
              markdown summary with an action items section. Every session is saved to a local
              encrypted library, so you can reopen one and keep chatting it through.
            </p>
          </div>
        </div>

        <div className="border-t border-rule pt-5 space-y-3">
          <div>
            <p className="text-sm font-medium text-heading">Yours, not rented</p>
            <p className="mt-1 text-xs leading-relaxed text-muted">
              No account, no subscription to forget to cancel, no backend quietly billing you for
              tokens. StageWhisper is a surface for the intelligence you already trained, not another
              thing phoning home. The source is public, so you can read exactly what it does instead
              of trusting a privacy page.
            </p>
          </div>
        </div>

        <div className="border-t border-rule pt-5 space-y-3">
          <div>
            <p className="text-sm font-medium text-heading">Privacy</p>
            <p className="mt-1 text-xs leading-relaxed text-muted">
              Your audio never leaves this machine, and StageWhisper itself has no backend or usage
              tracking. The transcript is sent only to the relay you configure: point it at an
              assistant running on this machine and it stays local; point it at a remote host and the
              transcript travels there, so choose your relay accordingly.
            </p>
          </div>
        </div>
      </div>

      {appVersion && (
        <div className="border-t border-rule px-6 py-3">
          <p className="text-xs text-faint">Version {appVersion}</p>
        </div>
      )}
    </div>
  );
}
