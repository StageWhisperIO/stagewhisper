import React from "react";
import ReactDOM from "react-dom/client";
import "../index.css";
import { initTheme } from "@/hooks/useTheme";
import { SettingsWindow } from "@/components/settings/window";
import { reportWebviewMounted } from "../lib/report-mounted";

initTheme();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsWindow />
  </React.StrictMode>,
);

reportWebviewMounted();
