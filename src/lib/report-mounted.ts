import { invoke } from "@tauri-apps/api/core";

const POLL_INTERVAL_MS = 50;
const MAX_ATTEMPTS = 200;

export function reportWebviewMounted() {
  let attempts = 0;
  const attempt = () => {
    attempts += 1;
    const retry = () => {
      if (attempts < MAX_ATTEMPTS) {
        setTimeout(attempt, POLL_INTERVAL_MS);
      } else {
        console.error("[report-mounted] giving up after repeated failures");
      }
    };
    const root = document.getElementById("root");
    if (root && root.childElementCount > 0) {
      invoke("webview_mounted").catch((error) => {
        console.error("[report-mounted] webview_mounted invoke failed:", error);
        retry();
      });
    } else {
      retry();
    }
  };
  attempt();
}
