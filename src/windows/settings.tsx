import React from "react";
import ReactDOM from "react-dom/client";
import "../index.css";
import { initTheme } from "@/hooks/useTheme";
import { SettingsWindow } from "@/components/settings/window";

initTheme();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <SettingsWindow />
  </React.StrictMode>,
);
