import type { ReactNode } from "react";
import { useLocalLlm } from "@/hooks/local-llm";
import { useModelReady } from "@/hooks/model-ready";
import { useRelayConnected } from "@/hooks/connection";
import { LocalPipelineSection } from "./local-pipeline";
import { LocalLlmSection } from "./local-llm";
import { EngineAssistantCard } from "./engine-assistant";

function StepBadge({ index, done }: { index: number; done: boolean }) {
  if (done) {
    return (
      <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-success-bg text-success">
        <svg
          width="15"
          height="15"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <polyline points="20 6 9 17 4 12" />
        </svg>
      </span>
    );
  }
  return (
    <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full border border-rule-strong text-sm font-semibold text-muted">
      {index}
    </span>
  );
}

function Step({
  index,
  title,
  subtitle,
  done,
  children,
}: {
  index: number;
  title: string;
  subtitle: string;
  done: boolean;
  children: ReactNode;
}) {
  return (
    <section className="space-y-4">
      <div className="flex items-start gap-3">
        <StepBadge index={index} done={done} />
        <div className="min-w-0">
          <h3 className="text-base font-semibold text-heading">{title}</h3>
          <p className="mt-0.5 text-sm text-muted">{subtitle}</p>
        </div>
      </div>
      <div className="pl-10">{children}</div>
    </section>
  );
}

export function EngineSection() {
  const { status, setPrimary } = useLocalLlm();
  const sttReady = useModelReady() === true;
  const assistantConnected = useRelayConnected() === true;
  const localReady = status?.ready === true;
  const primaryLocal = status?.primaryResponder === "local";
  const reasoningReady = primaryLocal ? localReady : assistantConnected;

  return (
    <div className="flex h-full flex-col overflow-hidden">
      <header className="border-b border-rule px-6 py-5">
        <h2 className="text-base font-semibold text-heading">Engine</h2>
        <p className="mt-0.5 text-sm text-muted">
          The pipeline behind your calls. Set up speech to text, then choose how StageWhisper
          answers.
        </p>
      </header>

      <div className="settings-scroll flex-1 overflow-y-auto px-6 py-6 space-y-10">
        <Step
          index={1}
          title="Speech to text"
          subtitle="Parakeet runs on this device and is always required. Audio never leaves your machine."
          done={sttReady}
        >
          <LocalPipelineSection embedded />
        </Step>

        <Step
          index={2}
          title="Reasoning"
          subtitle="Handles summaries and answers during calls. Choose an assistant or a local model."
          done={reasoningReady}
        >
          <div className="space-y-8">
            <div className="space-y-3 rounded-lg border border-rule-strong px-4 py-4">
              <div>
                <p className="text-sm font-medium text-heading">Which AI answers</p>
                <p className="mt-1 text-xs leading-relaxed text-muted">
                  Whichever you pick here runs your calls. Switch anytime.
                </p>
              </div>
              <div className="flex gap-2">
                <button
                  onClick={() => void setPrimary("external")}
                  className={`flex-1 rounded-lg border px-4 py-2 text-sm font-medium transition ${
                    !primaryLocal
                      ? "border-accent/40 bg-accent/10 text-accent"
                      : "border-rule-strong text-muted hover:border-accent/30 hover:text-body"
                  }`}
                >
                  Assistant
                </button>
                <button
                  onClick={() => void setPrimary("local")}
                  className={`flex-1 rounded-lg border px-4 py-2 text-sm font-medium transition ${
                    primaryLocal
                      ? "border-accent/40 bg-accent/10 text-accent"
                      : "border-rule-strong text-muted hover:border-accent/30 hover:text-body"
                  }`}
                >
                  Local model
                </button>
              </div>
            </div>

            {!primaryLocal && (
              <div className="space-y-4">
                <div className="flex items-center gap-2">
                  <p className="text-sm font-semibold text-heading">Assistant</p>
                  {assistantConnected && <span className="text-xs text-success">Connected</span>}
                </div>
                <EngineAssistantCard />
              </div>
            )}

            {primaryLocal && (
              <div className="space-y-4">
                <div className="flex items-center gap-2">
                  <p className="text-sm font-semibold text-heading">Local model</p>
                  {localReady && <span className="text-xs text-success">Ready</span>}
                </div>
                <LocalLlmSection embedded />
              </div>
            )}
          </div>
        </Step>
      </div>
    </div>
  );
}
