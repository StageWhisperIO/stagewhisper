import { scan } from "react-scan";
import React from "react";
import ReactDOM from "react-dom/client";
import "./index.css";
import { ControlPanel } from "./components/control-panel";
import { reportWebviewMounted } from "./lib/report-mounted";

scan({
  enabled: import.meta.env.DEV,
  showToolbar: false,
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ControlPanel />
  </React.StrictMode>,
);

reportWebviewMounted();
